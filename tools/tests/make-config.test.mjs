import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const makefile = join(root, "Makefile");
const make = execFileSync("/bin/sh", ["-c", "command -v make"], { encoding: "utf8" }).trim();

async function executable(path, contents) {
  await writeFile(path, contents);
  await chmod(path, 0o755);
}

async function readOrEmpty(path) {
  try {
    return await readFile(path, "utf8");
  } catch (error) {
    if (error.code === "ENOENT") return "";
    throw error;
  }
}

function parseLog(text) {
  return Object.fromEntries(text.trim().split("\n").map((line) => {
    const split = line.indexOf("=");
    return [line.slice(0, split), line.slice(split + 1)];
  }));
}

async function serveFixture({ failDev = false } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "librepaper-serve-config-"));
  const mockBin = join(directory, "mock-bin");
  const binary = join(directory, "dist/librepaper");
  const dbLog = join(directory, "db.log");
  const dockerLog = join(directory, "docker.log");
  const binaryLog = join(directory, "binary.log");
  await Promise.all([
    mkdir(mockBin, { recursive: true }),
    mkdir(join(directory, "dist"), { recursive: true }),
    mkdir(join(directory, "tools"), { recursive: true }),
  ]);
  await executable(join(directory, "tools/db"),
    "#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$DB_LOG\"\ncase \"$1\" in\n" +
    "dev) docker dev; " + (failDev ? "exit 37" : "exit 0") + ";;\n" +
    "url) printf '%s\\n' 'postgresql://dev:secret@127.0.0.1/librepaper';;\n" +
    "*) exit 2;;\nesac\n");
  await executable(join(mockBin, "docker"),
    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$DOCKER_LOG\"\n");
  await executable(binary,
    "#!/bin/sh\n{\n" +
    "printf 'args=%s\\n' \"$*\"\n" +
    "printf 'db=%s\\n' \"$LIBREPAPER_DATABASE_URL\"\n" +
    "printf 'port=%s\\n' \"$LIBREPAPER_PORT\"\n" +
    "printf 'data=%s\\n' \"$LIBREPAPER_DATA\"\n" +
    "printf 'app=%s\\n' \"$LIBREPAPER_APP_ORIGIN\"\n" +
    "printf 'site=%s\\n' \"$LIBREPAPER_SITE_ORIGIN\"\n" +
    "printf 'activity=%s\\n' \"$LIBREPAPER_SIMULATE_ACTIVITY\"\n" +
    "} > \"$BINARY_LOG\"\n");
  const environment = {
    ...process.env,
    PATH: mockBin + ":" + process.env.PATH,
    MAKEFLAGS: "",
    MFLAGS: "",
    MAKELEVEL: "",
    DB_LOG: dbLog,
    DOCKER_LOG: dockerLog,
    BINARY_LOG: binaryLog,
  };
  delete environment.CONFIG;
  delete environment.LIBREPAPER_DATABASE_URL;
  delete environment.LIBREPAPER_PORT;
  delete environment.LIBREPAPER_DATA;
  delete environment.LIBREPAPER_APP_ORIGIN;
  delete environment.LIBREPAPER_SITE_ORIGIN;
  delete environment.LIBREPAPER_SIMULATE_ACTIVITY;
  return {
    directory, binary, dbLog, dockerLog, binaryLog, environment,
    cleanup: () => rm(directory, { recursive: true, force: true }),
  };
}

function runServe(fixture, config, extraEnvironment = {}) {
  const args = [
    "--file=" + makefile, "--old-file=" + fixture.binary, "--no-print-directory",
    "SHELL=/bin/sh", "BIN=" + fixture.binary, "OPEN=0",
  ];
  if (config !== undefined) args.push("CONFIG=" + config);
  args.push("serve");
  return spawnSync(make, args, {
    cwd: fixture.directory,
    encoding: "utf8",
    timeout: 20000,
    env: { ...fixture.environment, ...extraEnvironment },
  });
}

test("custom serve uses its config and inherited settings without starting Docker", async () => {
  const fixture = await serveFixture();
  try {
    const result = runServe(fixture, "custom.toml", {
      LIBREPAPER_PORT: "5100",
      LIBREPAPER_DATA: "/inherited/data",
      LIBREPAPER_APP_ORIGIN: "https://inherited.example",
      LIBREPAPER_SITE_ORIGIN: "https://site.inherited.example",
      LIBREPAPER_SIMULATE_ACTIVITY: "4",
    });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const values = parseLog(await readFile(fixture.binaryLog, "utf8"));
    assert.equal(values.args, "admin serve --config custom.toml");
    assert.equal(values.db, "");
    assert.equal(values.port, "5100");
    assert.equal(values.data, "/inherited/data");
    assert.equal(values.app, "https://inherited.example");
    assert.equal(values.site, "https://site.inherited.example");
    assert.equal(values.activity, "4");
    assert.equal(await readOrEmpty(fixture.dbLog), "");
    assert.equal(await readOrEmpty(fixture.dockerLog), "");
  } finally {
    await fixture.cleanup();
  }
});

for (const config of [undefined, "tools/dev-oauth.toml"]) {
  test("serve bootstraps " + (config || "the default dev config"), async () => {
    const fixture = await serveFixture();
    try {
      const result = runServe(fixture, config, {
        PORT: "6181",
        DATA: "fixture-data",
        SIMULATE_ACTIVITY: "13",
      });
      assert.equal(result.status, 0, result.stderr || result.stdout);
      const values = parseLog(await readFile(fixture.binaryLog, "utf8"));
      assert.equal(values.args, "admin serve --config " + (config || "tools/dev.toml"));
      assert.equal(values.db, "postgresql://dev:secret@127.0.0.1/librepaper");
      assert.equal(values.port, "6181");
      assert.equal(values.data, join(fixture.directory, "fixture-data"));
      assert.equal(values.app, "http://localhost:6181");
      assert.equal(values.site, "http://localhost:6181");
      assert.equal(values.activity, "13");
      assert.equal(await readFile(fixture.dbLog, "utf8"), "dev\nurl\n");
      assert.equal(await readFile(fixture.dockerLog, "utf8"), "dev\n");
    } finally {
      await fixture.cleanup();
    }
  });
}

test("a failed dev database bootstrap stops before the server starts", async () => {
  const fixture = await serveFixture({ failDev: true });
  try {
    const result = runServe(fixture, undefined);
    assert.notEqual(result.status, 0);
    assert.equal(await readFile(fixture.dbLog, "utf8"), "dev\n");
    assert.equal(await readFile(fixture.dockerLog, "utf8"), "dev\n");
    assert.equal(await readOrEmpty(fixture.binaryLog), "");
  } finally {
    await fixture.cleanup();
  }
});

async function demoFixture({ configEnvironment, githubId, decryptable = false } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "librepaper-demo-config-"));
  const mockBin = join(directory, "mock-bin");
  const makeLog = join(directory, "make.log");
  const sopsLog = join(directory, "sops.log");
  await mkdir(mockBin, { recursive: true });
  const mockMake = join(mockBin, "mock-make");
  await executable(mockMake, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$MAKE_LOG\"\n");
  await executable(join(mockBin, "sops"),
    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$SOPS_LOG\"\n" +
    "if [ \"$1\" = --decrypt ]; then [ \"$KEYS_DECRYPTABLE\" = 1 ]; exit $?; fi\n" +
    "if [ \"$1\" = exec-env ]; then /bin/sh -c \"$3\"; exit $?; fi\nexit 2\n");
  const environment = {
    ...process.env,
    PATH: mockBin + ":" + process.env.PATH,
    MAKEFLAGS: "",
    MFLAGS: "",
    MAKELEVEL: "",
    MAKE_LOG: makeLog,
    SOPS_LOG: sopsLog,
    KEYS_DECRYPTABLE: decryptable ? "1" : "0",
  };
  delete environment.CONFIG;
  delete environment.LIBREPAPER_GITHUB_CLIENT_ID;
  if (configEnvironment !== undefined) environment.CONFIG = configEnvironment;
  if (githubId !== undefined) environment.LIBREPAPER_GITHUB_CLIENT_ID = githubId;
  return {
    directory, mockMake, makeLog, sopsLog, environment,
    cleanup: () => rm(directory, { recursive: true, force: true }),
  };
}

function runDemo(fixture, commandLineConfig) {
  const args = [
    "--file=" + makefile, "--no-print-directory", "SHELL=/bin/sh",
    "MAKE=" + fixture.mockMake, "DEMO_KEYS=" + join(fixture.directory, "keys.yaml"),
  ];
  if (commandLineConfig !== undefined) args.push("CONFIG=" + commandLineConfig);
  args.push("demo");
  return spawnSync(make, args, {
    cwd: fixture.directory,
    encoding: "utf8",
    timeout: 20000,
    env: fixture.environment,
  });
}

async function demoLogs(fixture) {
  return {
    make: await readOrEmpty(fixture.makeLog),
    sops: await readOrEmpty(fixture.sopsLog),
  };
}

test("demo defaults to OAuth config when SOPS keys decrypt", async () => {
  const fixture = await demoFixture({ decryptable: true });
  try {
    const result = runDemo(fixture);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const logs = await demoLogs(fixture);
    assert.match(logs.make, /demo-run CONFIG=tools\/dev-oauth\.toml/);
    assert.match(logs.sops, /--decrypt/);
    assert.match(logs.sops, /exec-env .*CONFIG=tools\/dev-oauth\.toml/);
  } finally {
    await fixture.cleanup();
  }
});

test("demo defaults to no-OAuth config when SOPS keys are unavailable", async () => {
  const fixture = await demoFixture();
  try {
    const result = runDemo(fixture);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const logs = await demoLogs(fixture);
    assert.match(logs.make, /demo-run CONFIG=tools\/dev\.toml/);
    assert.match(logs.sops, /--decrypt/);
    assert.doesNotMatch(logs.sops, /exec-env/);
  } finally {
    await fixture.cleanup();
  }
});

test("demo chooses OAuth config when a GitHub client ID is inherited", async () => {
  const fixture = await demoFixture({ githubId: "provided-id" });
  try {
    const result = runDemo(fixture);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const logs = await demoLogs(fixture);
    assert.match(logs.make, /demo-run CONFIG=tools\/dev-oauth\.toml/);
    assert.equal(logs.sops, "");
  } finally {
    await fixture.cleanup();
  }
});

for (const selected of [
  { source: "command line", value: "custom.toml", commandLine: true },
  { source: "environment", value: "tools/dev.toml", commandLine: false },
]) {
  test("demo preserves explicit CONFIG from " + selected.source, async () => {
    const fixture = await demoFixture({ decryptable: true });
    if (!selected.commandLine) fixture.environment.CONFIG = selected.value;
    try {
      const result = runDemo(fixture, selected.commandLine ? selected.value : undefined);
      assert.equal(result.status, 0, result.stderr || result.stdout);
      const logs = await demoLogs(fixture);
      assert.ok(logs.make.includes("demo-run CONFIG=" + selected.value));
      assert.equal(logs.sops, "");
    } finally {
      await fixture.cleanup();
    }
  });
}
