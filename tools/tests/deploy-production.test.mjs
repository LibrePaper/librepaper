import assert from "node:assert/strict";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const sourceScript = fileURLToPath(new URL("../deploy-production", import.meta.url));
const node = process.execPath;
const credentials = {
  PRODUCTION_ACME_EMAIL: "ops@example.invalid",
  PRODUCTION_POSTGRES_PASSWORD: "postgres-secret-test-value",
  PRODUCTION_GITHUB_CLIENT_ID: "github-client-id-test-value",
  PRODUCTION_GITHUB_CLIENT_SECRET: "github-client-secret-test-value",
  PRODUCTION_GOOGLE_CLIENT_ID: "google-client-id-test-value",
  PRODUCTION_GOOGLE_CLIENT_SECRET: "google-client-secret-test-value",
};

async function fixture(t, { values = credentials, googleStatus = "302", googleLocation = "https://accounts.google.com/o/oauth2/v2/auth?client_id=test", sopsFailKey = "" } = {}) {
  const root = await mkdtemp(join(tmpdir(), "deploy-production-oauth-"));
  const bin = join(root, "bin");
  await mkdir(join(root, "tools", "deploy-docker"), { recursive: true });
  await mkdir(join(root, "crates", "librepaper"), { recursive: true });
  await mkdir(join(root, "docs"), { recursive: true });
  await mkdir(bin);
  await writeFile(join(root, "tools", "deploy-production"), await readFile(sourceScript));
  await writeFile(join(root, "tools", "deploy-keys.yaml"), "test fixture; SOPS is mocked\n");
  await writeFile(join(root, "crates", "librepaper", "Cargo.toml"), 'version = "0.1.2"\n');

  const commands = join(root, "commands.log");
  const envFile = join(root, "remote.env");
  const curlCalls = join(root, "curl-calls.jsonl");
  const sops = join(bin, "sops");
  await writeFile(sops, `#!${node}
const fs = require("node:fs");
const args = process.argv.slice(2);
const extract = args[args.indexOf("--extract") + 1] || "";
const key = extract.match(/PRODUCTION_[A-Z_]+/)?.[0];
const values = JSON.parse(process.env.TEST_SOPS_VALUES || "{}");
if (key === process.env.TEST_SOPS_FAIL_KEY) { process.stderr.write("private SOPS decryption diagnostic"); process.exit(9); }
if (Object.hasOwn(values, key)) process.stdout.write(values[key] === null ? "null" : String(values[key]));
`);
  const make = join(bin, "make");
  const rsync = join(bin, "rsync");
  const ssh = join(bin, "ssh");
  const dig = join(bin, "dig");
  const curl = join(bin, "curl");
  const logCommand = (name) => `#!${node}\nrequire("node:fs").appendFileSync(process.env.TEST_COMMANDS, ${JSON.stringify(name + "\n")});\n`;
  await writeFile(make, logCommand("make"));
  await writeFile(rsync, logCommand("rsync"));
  await writeFile(ssh, `#!${node}
const fs = require("node:fs");
const args = process.argv.slice(2);
fs.appendFileSync(process.env.TEST_COMMANDS, "ssh\\n");
if (args.join(" ").includes("librepaper/.env")) fs.writeFileSync(process.env.TEST_ENV_FILE, fs.readFileSync(0));
`);
  await writeFile(dig, `#!${node}\nprocess.stdout.write("203.0.113.10\\n");\n`);
  await writeFile(curl, `#!${node}
const args = process.argv.slice(2);
const url = args.at(-1) || "";
let status = "200", location = "";
if (url.includes("/auth/login/github")) { status = "302"; location = "https://github.com/login/oauth/authorize?client_id=test"; }
if (url.includes("/auth/login/google")) {
  status = process.env.TEST_GOOGLE_STATUS || "302";
  location = Object.hasOwn(process.env, "TEST_GOOGLE_LOCATION") ? process.env.TEST_GOOGLE_LOCATION : "https://accounts.google.com/o/oauth2/v2/auth?client_id=test";
}
if (url.includes("/auth/login/")) require("node:fs").appendFileSync(process.env.TEST_CURL_CALLS, JSON.stringify({ url, args }) + "\\n");
if (url === "https://librepaper.com") { process.stdout.write("HTTP/2 301\\r\\nLocation: https://librepaper.org/\\r\\n\\r\\n"); process.exit(0); }
const formatAt = args.indexOf("-w");
if (formatAt >= 0) {
  const format = args[formatAt + 1].replaceAll("\\\\t", "\\t").replaceAll("\\\\n", "\\n");
  process.stdout.write(format.replaceAll("%{http_code}", status).replaceAll("%{redirect_url}", location));
}
`);
  for (const path of [sops, make, rsync, ssh, dig, curl]) await chmod(path, 0o755);
  t.after(() => rm(root, { recursive: true, force: true }));
  return {
    root,
    script: join(root, "tools", "deploy-production"),
    commands,
    curlCalls,
    envFile,
    run(args, overrides = {}) {
      return execFileSync("bash", [join(root, "tools", "deploy-production"), ...args], {
        cwd: root,
        encoding: "utf8",
        stdio: "pipe",
        timeout: 10_000,
        env: {
          ...process.env,
          PATH: `${bin}:${process.env.PATH}`,
          TEST_COMMANDS: commands,
          TEST_CURL_CALLS: curlCalls,
          TEST_ENV_FILE: envFile,
          TEST_SOPS_VALUES: JSON.stringify(values),
          TEST_SOPS_FAIL_KEY: sopsFailKey,
          TEST_GOOGLE_STATUS: googleStatus,
          TEST_GOOGLE_LOCATION: googleLocation,
          HOST: "deploy-test.invalid",
          ...overrides,
        },
      });
    },
  };
}

async function captureFailure(run) {
  try {
    run();
  } catch (error) {
    return `${error.stdout || ""}${error.stderr || ""}`;
  }
  assert.fail("expected deployment command to fail");
}

test("deploy writes mapped Google OAuth credentials and keeps secrets out of logs", async (t) => {
  const f = await fixture(t);
  const output = f.run(["deploy", "v0.1.2"]);
  const env = await readFile(f.envFile, "utf8");
  assert.match(env, /^LIBREPAPER_GOOGLE_CLIENT_ID=google-client-id-test-value$/m);
  assert.match(env, /^LIBREPAPER_GOOGLE_CLIENT_SECRET=google-client-secret-test-value$/m);
  assert.match(output, /6 keys decrypted/);
  assert.match(output, /12 settings/);
  for (const value of Object.values(credentials)) {
    assert.ok(!output.includes(value), `deployment output leaked ${value}`);
  }
});

test("SOPS decryption failures stop before build or remote changes and suppress SOPS diagnostics", async (t) => {
  const f = await fixture(t, { sopsFailKey: "PRODUCTION_GOOGLE_CLIENT_SECRET" });
  const output = await captureFailure(() => f.run(["deploy", "v0.1.2"]));
  assert.match(output, /PRODUCTION_GOOGLE_CLIENT_SECRET/);
  assert.doesNotMatch(output, /private SOPS decryption diagnostic|google-client-secret-test-value/);
  await assert.rejects(readFile(f.commands));
});

test("missing, null, empty, and whitespace-only Google credentials stop before build or remote changes", async (t) => {
  for (const [key, value] of [
    ["PRODUCTION_GOOGLE_CLIENT_ID", undefined],
    ["PRODUCTION_GOOGLE_CLIENT_SECRET", null],
    ["PRODUCTION_GOOGLE_CLIENT_ID", ""],
    ["PRODUCTION_GOOGLE_CLIENT_SECRET", "   \t"],
  ]) {
    await t.test(`${key} ${value === undefined ? "missing" : JSON.stringify(value)}`, async (t) => {
      const values = { ...credentials };
      if (value === undefined) delete values[key];
      else values[key] = value;
      const f = await fixture(t, { values });
      const output = await captureFailure(() => f.run(["deploy", "v0.1.2"]));
      assert.match(output, new RegExp(key));
      assert.doesNotMatch(output, /google-client-(?:id|secret)-test-value|postgres-secret-test-value/);
      await assert.rejects(readFile(f.commands));
    });
  }
});

test("verify accepts GitHub and Google authorization redirects without following them", async (t) => {
  const f = await fixture(t);
  const output = f.run(["verify"]);
  assert.match(output, /GitHub OAuth/i);
  assert.match(output, /Google OAuth/i);
  const calls = (await readFile(f.curlCalls, "utf8")).trim().split("\n").map(JSON.parse);
  assert.equal(calls.length, 2);
  assert.ok(calls.every(({ args }) => !args.includes("-L") && !args.includes("--location")));
});

test("verify rejects absent and wrong-provider Google authorization redirects without exposing their URLs", async (t) => {
  for (const scenario of [
    { name: "missing redirect", googleStatus: "200", googleLocation: "" },
    { name: "not found with no Location", googleStatus: "404", googleLocation: "" },
    { name: "GitHub provider redirect", googleStatus: "302", googleLocation: "https://github.com/login/oauth/authorize?client_id=test" },
  ]) {
    await t.test(scenario.name, async (t) => {
      const f = await fixture(t, scenario);
      const output = await captureFailure(() => f.run(["verify"]));
      assert.match(output, /Google/i);
      assert.doesNotMatch(output, /accounts\.google\.com|github\.com/);
    });
  }
});
