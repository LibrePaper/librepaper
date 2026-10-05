// End-to-end browser check for the no-retained-renderings contract.
//
//   node web/tools/latex-e2e.mjs <binary> <mode> <fixture> [seconds] [mirror]
//
// `mode` is `browser` (the reader's browser compiler) or `local` (the same
// reader check, with an optional local companion available for fallback). A
// reader opens the published read link, so its only document authority is the
// fragment key. Publishing itself uses a locally minted, signed GitHub test
// session; no OAuth service is contacted.
import { createHash, randomBytes } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "./browser-driver.mjs";
import { ephemeralMirror } from "./ephemeral-mirror.mjs";
import { postgresTestDatabase } from "./postgres-test.mjs";
import { sessionCookie } from "../tests/helpers/deployment.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const BINARY = resolve(process.argv[2] || "target/debug/librepaper");
const MODE = process.argv[3] || "browser";
const REQUESTED_FIXTURE = resolve(process.argv[4] || join(ROOT, "tools", "test", "latex", "corpus", "e2e", "biber"));
const WAIT = Number(process.argv[5] || 300);
const MIRROR_ARG = process.argv[6] || process.env.MIRROR || join(ROOT, "..", "wasm-latex", "mirror");
const PORT = 8600 + Math.floor(Math.random() * 200);
const BASE = `http://localhost:${PORT}`;
const scratch = mkdtempSync(join(tmpdir(), "librepaper-latex-e2e-"));
const data = mkdtempSync(join(tmpdir(), "librepaper-latex-data-"));
const config = mkdtempSync(join(tmpdir(), "librepaper-latex-config-"));
let mirror = null;
let server = null;
let local = null;
let postgres = null;

const wait = (ms) => new Promise((done) => setTimeout(done, ms));
const shellHeaders = { "x-librepaper-client": "shell" };

function fixtureTree(directory, main) {
  const files = (dir, prefix = "") => readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name.startsWith(".") || entry.name === "logs") return [];
    const relative = prefix + entry.name;
    if (entry.isSymbolicLink()) throw new Error(`fixture symlink: ${relative}`);
    return entry.isDirectory() ? files(join(dir, entry.name), `${relative}/`) : [relative];
  });
  const form = new FormData();
  form.set("title", "LaTeX browser smoke");
  form.set("main", main);
  for (const path of files(directory).sort()) {
    form.append("file", new Blob([readFileSync(join(directory, path))]), path);
  }
  return form;
}

async function publish(cookie, fixture) {
  // A directory is published with main.tex as its main file; a .tex file is
  // published with everything beside it (its \input files, figures and
  // bibliography), as an author uploading that project would.
  const body = statSync(fixture).isDirectory()
    ? fixtureTree(fixture, "main.tex")
    : fixtureTree(dirname(fixture), basename(fixture));
  const headers = { ...shellHeaders, cookie };
  const response = await fetch(`${BASE}/api/documents`, { method: "POST", headers, body });
  const value = await response.json();
  if (!response.ok || typeof value.share_url !== "string") throw new Error(`publish failed (${response.status}): ${JSON.stringify(value)}`);
  const link = new URL(value.share_url, BASE);
  const key = new URLSearchParams(link.hash.slice(1)).get("k");
  if (!key) throw new Error(`publish returned no reader key: ${link}`);
  return { url: link.href, slug: link.pathname.split("/").filter(Boolean).pop(), key };
}

async function main() {
  postgres = postgresTestDatabase("latex_e2e");
  if (!(MODE === "browser" || MODE === "local")) throw new Error(`unknown mode ${MODE}; use browser or local`);
  const fixture = existsSync(REQUESTED_FIXTURE)
    ? REQUESTED_FIXTURE
    : join(ROOT, "tools", "test", "latex", "corpus", "e2e", "native");
  if (!statSync(fixture)) throw new Error(`fixture does not exist: ${fixture}`);

  const mirrorUrl = /^https:\/\//i.test(MIRROR_ARG)
    ? MIRROR_ARG.replace(/\/?$/, "/")
    : (mirror = await ephemeralMirror(MIRROR_ARG)).latexUrl;
  // The server takes the asset mirror; the pinned release is beneath it at latex/<id>/.
  const assetMirror = new URL("../../", mirrorUrl).href;
  const serverConfig = join(config, "server.toml");
  writeFileSync(serverConfig, [
    "[server]",
    `address = "0.0.0.0:${PORT}"`,
    "local_companion = true",
    "",
    "[storage]",
    `directory = ${JSON.stringify(data)}`,
    'database_url = { env = "LIBREPAPER_DATABASE_URL" }',
    "",
    "[auth.github]",
    'client_id = { env = "LIBREPAPER_GITHUB_CLIENT_ID" }',
    'client_secret = { env = "LIBREPAPER_GITHUB_CLIENT_SECRET" }',
    "",
    "[access]",
    'publishers = ["any"]',
    'commenters = ["anyone"]',
    "",
    "[assets]",
    `mirror = ${JSON.stringify(assetMirror)}`,
    "",
  ].join("\n"));
  server = spawn(BINARY, ["admin", "serve", "--config", serverConfig], {
    stdio: ["ignore", "ignore", "pipe"],
    env: { ...process.env, LIBREPAPER_DATABASE_URL: postgres.url, LIBREPAPER_GITHUB_CLIENT_ID: "test-client", LIBREPAPER_GITHUB_CLIENT_SECRET: "test-secret" },
  });
  let serverLog = "";
  server.stderr.on("data", (bytes) => { serverLog += String(bytes); });
  await until("serve startup", async () => {
    if (server.exitCode !== null) throw new Error(serverLog);
    return (await fetch(`${BASE}/api/config`)).ok;
  }, 30000);
  console.log("origin baseline " + JSON.stringify(await (await fetch(`${BASE}/api/status`)).json()));
  // A real sign-in: the account row is seeded first, and the cookie names it by
  // id, so the binary checks provider, account and session generation.
  const accountId = postgres.seedRegisteredAccount({ provider: "github", subject: "github:browser-test", handle: "browser-test", displayName: "Browser Test" });
  const cookie = sessionCookie({ dataDirectory: data, accountId, handle: "browser-test", name: "Browser Test" });
  const published = await publish(cookie, fixture);
  console.log(`published ${published.slug}; reader key only; mirror ${mirrorUrl}`);

  const targetUrl = published.url;
  let localPairing = null;
  if (MODE === "local") {
    // No display: the companion asks on its terminal, and the approve
    // command answers as the person at this computer would.
    const env = { ...process.env, XDG_CONFIG_HOME: config, XDG_CACHE_HOME: join(config, "cache") };
    delete env.DISPLAY;
    delete env.WAYLAND_DISPLAY;
    let log = "";
    local = spawn(BINARY, ["start", "--foreground", "--port", "8763"], { stdio: ["ignore", "ignore", "pipe"], env });
    local.stderr.on("data", (data) => { log += data; });
    const base = "http://127.0.0.1:8763/librepaper/local";
    await until("local app ready", async () => {
      try { return (await fetch(`${base}/health`)).ok; } catch { return false; }
    }, 30000);
    const health = await (await fetch(`${base}/health`)).json();
    const request = randomBytes(24).toString("base64url");
    const verifier = randomBytes(32).toString("base64url");
    const challenge = createHash("sha256").update(verifier).digest("hex");
    const headers = { "content-type": "application/json", origin: BASE };
    const asked = await fetch(`${base}/pair/request`, {
      method: "POST", headers,
      body: JSON.stringify({ origin: BASE, project: published.slug, request, challenge, return: BASE }),
    });
    if (asked.status !== 202) throw new Error(`pair/request failed (${asked.status}): ${await asked.text()}`);
    let code = null;
    await until("approval asked on the terminal", async () => {
      code = log.match(/librepaper local approve (\d{6})/)?.[1] || null;
      return Boolean(code);
    }, 30000);
    const approved = spawnSync(BINARY, ["local", "approve", code], { env, encoding: "utf8" });
    if (approved.status !== 0) throw new Error(`local approve failed: ${approved.stderr}`);
    let pairing = null;
    await until("pairing claimed", async () => {
      const claimed = await fetch(`${base}/connect/claim`, {
        method: "POST", headers, body: JSON.stringify({ request, origin: BASE, project: published.slug, verifier }),
      });
      if (claimed.status === 202) return false;
      pairing = await claimed.json();
      if (!claimed.ok || !pairing.token) throw new Error(`local pairing failed (${claimed.status}): ${JSON.stringify(pairing)}`);
      return true;
    }, 30000);
    localPairing = { token: pairing.token, expires: pairing.expires, instance: health.instance };
    // The local mode keeps the companion fallback reachable for the Biber
    // fixture. Browser Biber remains preferred by the renderer; the local
    // pairing is selected only after browser resource failure.
  }

  process.env.LIBREPAPER_BROWSER_IGNORE_CERT_ERRORS = "1";
  const tab = await browser(process.env.BROWSER || "chromium", join(scratch, "profile"), 9500 + Math.floor(Math.random() * 500));
  try {
    await tab.resize(1300, 900);
    if (localPairing) {
      await tab.navigate(BASE);
      await tab.evaluate(`localStorage.setItem("librepaper-local-pairings", JSON.stringify({ [location.origin + "|" + ${JSON.stringify(published.slug)}]: ${JSON.stringify(localPairing)} }))`);
    }
    // A wait that, when it runs out, says what the reader was showing and
    // which mirror files it fetched, rather than only that time ran out.
    let seen = "";
    const explained = (label, check) => until(label, check, WAIT * 1000).catch(async (error) => {
      const page = await tab.evaluate("document.body.innerText").catch(() => "");
      const fetched = await tab.evaluate(`performance.getEntriesByType("resource")
        .filter((entry) => /latex|wasm/.test(entry.name))
        .map((entry) => entry.responseStatus + " " + entry.name.replace(/^https?:\\/\\/[^/]+/, ""))
        .join("\\n")`).catch(() => "");
      const squash = (value) => String(value).replace(/\s+/g, " ").slice(0, 600);
      throw new Error(`${error.message}; document: ${squash(seen)}; reader page: ${squash(page)}\nmirror requests:\n${fetched}`);
    });

    // LaTeX opens as the LaTeXML HTML preview.
    await tab.navigate(targetUrl);
    const started = Date.now();
    await explained("reader HTML preview", async () => {
      const text = seen = await tab.text().catch(() => "");
      return /Knuth|LaTeX|browser|paragraph/i.test(text) && !/not yet rendered|rendering from source/i.test(text);
    });
    console.log(`reader HTML preview drawn in ${((Date.now() - started) / 1000).toFixed(1)}s`);

    // A reader is shown LaTeX as HTML; choosing pages is an editor's View menu.
    if ((fixture.endsWith("/biber") || fixture.endsWith("\\biber")) && !/Knuth/i.test(seen)) {
      throw new Error("Biber fixture rendered without its resolved Knuth citation");
    }

    const resources = await tab.evaluate("performance.getEntriesByType('resource').map(e => e.name)");
    const origin = new URL(BASE).origin;
    if (resources.some((url) => new URL(url).origin === origin && new URL(url).pathname.startsWith("/latex/"))) throw new Error("reader fetched compiler bytes through the origin /latex route");
    if (!resources.some((url) => url.startsWith(mirrorUrl))) throw new Error(`reader did not fetch compiler assets directly from ${mirrorUrl}`);
    console.log("cold reader used direct HTTPS mirror and no origin /latex route");

    const readHeaders = { ...shellHeaders, "x-librepaper-key": published.key };
    for (const path of [`/api/documents/${published.slug}/renderings/latest`, `/api/documents/${published.slug}/renderings/${"0".repeat(64)}`]) {
      const response = await fetch(`${BASE}${path}`, { headers: readHeaders });
      if (response.status !== 404) throw new Error(`rendered output route ${path} returned ${response.status}, expected 404`);
    }
    console.log("origin has no retained rendering route");
    console.log("origin after cold compile " + JSON.stringify(await (await fetch(`${BASE}/api/status`)).json()));
  } finally {
    await tab.close();
  }
}

try {
  await main();
} catch (error) {
  console.error(`E2E FAILED: ${error.message}`);
  process.exitCode = 1;
} finally {
  server?.kill();
  local?.kill("SIGINT");
  postgres?.drop();
  mirror?.server.close();
  await wait(250);
  for (const directory of [data, config, scratch, mirror?.tls].filter(Boolean)) rmSync(directory, { recursive: true, force: true, maxRetries: 3 });
}
