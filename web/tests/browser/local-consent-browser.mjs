// The one connection, end to end in a real browser against a real
// `librepaper local start`: the browser asks the local app to connect,
// a system dialog appears to approve (or a command-line approve on machines
// without a display), and the client ends up connected with the hosted binding
// without anything typed or bound.
//
// This test runs headless, so the companion prints the approve code to stderr.
// The test extracts it, runs the approve command, and waits for the connection.
//
// Needs `dist/librepaper` (run `make build`) and chromium on PATH.

import assert from "node:assert/strict";
import { build } from "vite";
import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:http";

import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const binary = process.env.LIBREPAPER_TEST_BINARY || join(root, "dist", "librepaper");
assert.ok(existsSync(binary), "run `make build` first");

const temporary = mkdtempSync(join(tmpdir(), "librepaper-consent-check-"));
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
async function freePort() {
  const server = createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}
const appPort = await freePort();
let pagePort = 0;
const appAddress = `http://127.0.0.1:${appPort}/`;

writeFileSync(entry, `
import * as local from ${JSON.stringify(join(root, "web/src/lib/companion/client.js"))};
window.local = local;
window.errors = [];
addEventListener("error", (event) => window.errors.push(String(event.message)));
addEventListener("unhandledrejection", (event) => window.errors.push(String(event.reason)));
local.configure({ project: "paper-check", origin: location.origin });
local.setAddress(${JSON.stringify(appAddress)});
window.pairing = null;
window.startPairing = () => { window.pairing = local.connectApp().then((status) => ({ state: status.state, error: status.error })); };
`);

await build({
  configFile: false,
  root: join(root, "web"),
  logLevel: "error",
  build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js" } },
});

const page = `<!doctype html><html><head><meta charset="utf-8"><title>consent check</title></head><body><script type="module" src="/check.js"></script></body></html>`;
const server = createServer((request, response) => {
  if (request.url === "/check.js") {
    response.setHeader("content-type", "text/javascript");
    response.end(readFileSync(join(output, "check.js")));
    return;
  }
  response.setHeader("content-type", "text/html");
  response.end(page);
});
await new Promise((resolve) => server.listen(pagePort, "127.0.0.1", resolve));
pagePort = server.address().port;

// The local app, with its state and cache kept out of the real home, and no
// display, so approval falls to the terminal code a test can read.
const stateHome = join(temporary, "state");
const cacheHome = join(temporary, "cache");
const appEnv = { ...process.env, XDG_STATE_HOME: stateHome, XDG_CACHE_HOME: cacheHome, HOME: temporary };
delete appEnv.DISPLAY;
delete appEnv.WAYLAND_DISPLAY;
const app = spawn(binary, ["local", "start", "--port", String(appPort)], { env: appEnv, stdio: ["ignore", "pipe", "pipe"] });
let appLog = "";
app.stdout.on("data", (chunk) => (appLog += chunk));
app.stderr.on("data", (chunk) => (appLog += chunk));
await until("local app health", async () => {
  const response = await fetch(`${appAddress}librepaper/local/health`);
  return response.ok;
}, 15000);

let b;
try {
  b = await browser("chromium", join(temporary, "chrome"), await freePort());
  await b.navigate(`http://localhost:${pagePort}/`);
  await until("client loaded", () => b.evaluate("Boolean(window.local)"), 10000);
  assert.equal(await b.evaluate("window.local.bindingId()"), "hosted", "the hosted binding is the default");

  // Nothing is paired yet: the app is reachable but has not allowed this site.
  const before = await b.evaluate("window.local.retry().then((s) => s.state)");
  assert.equal(before, "unauthorized", `before consent: ${before}`);

  // Connect asks the companion directly; no window opens. With no display
  // the companion asks on its terminal instead of in a dialog, and the
  // approve command answers it exactly as the person at this computer would.
  await b.evaluate("window.startPairing()");
  let approveCode = null;
  await until("approval asked on the terminal", () => {
    approveCode = appLog.match(/librepaper local approve (\d{6})/)?.[1] || null;
    return Boolean(approveCode);
  }, 10000);
  assert.equal(await b.evaluate("window.local.status().state"), "unauthorized", "nothing is paired before approval");
  const approved = spawnSync(binary, ["local", "approve", approveCode], { env: appEnv, encoding: "utf8" });
  assert.equal(approved.status, 0, `approve: ${approved.stderr}`);

  // Wait on the client's status rather than the pairing promise, so a
  // failure reports what the page saw instead of hanging on a promise.
  try {
    await until("pairing arrives", () => b.evaluate("window.local.status().state === 'connected'"), 15000);
  } catch (error) {
    const status = await b.evaluate("JSON.stringify(window.local.status())");
    throw new Error(`${error.message}\nstatus: ${status}\napp log: ${appLog}`);
  }
  const outcome = "window.pairing.then((r) => JSON.stringify(r))";
  const after = JSON.parse(await b.evaluate(outcome));
  assert.equal(after.state, "connected", `after consent: ${JSON.stringify(after)} app log: ${appLog}`);
  assert.deepEqual(await b.evaluate("window.errors"), []);

  // The pairing is stored and serves authenticated calls from now on, and
  // was issued for the reader's origin only.
  const caps = await b.evaluate("window.local.capabilities().then((c) => Boolean(c && c.tools))");
  assert.equal(caps, true, "capabilities answer with the new token");
  const pairings = JSON.parse(readFileSync(join(stateHome, "librepaper", "local", "pairings.json"), "utf8"));
  const keys = Object.keys(pairings);
  assert.equal(keys.length, 1, `one pairing: ${keys}`);
  assert.match(keys[0], new RegExp(`^http://localhost:${pagePort}\\|paper-check$`));

  console.log("local-consent-browser: hosted binding by default, Connect asks the companion with no window, terminal approval pairs and connects, one pairing for the reader's origin passed");
} finally {
  if (b) await b.close();
  app.kill();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
