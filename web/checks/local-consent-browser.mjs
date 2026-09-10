// The one-click pairing, end to end in a real browser against a real
// `librepaper local start`: the reader asks the local app for permission, the
// app's consent page is answered with Allow, the pairing comes back by
// postMessage, and the client ends up connected with the hosted binding --
// nothing typed, nothing bound.
//
// The consent page is opened in an iframe rather than the popup the reader
// uses, because a headless session cannot reach a popup's window; the page
// answers a parent frame exactly as it answers an opener. The reader page is
// served on `localhost` and the app on `127.0.0.1` so the frame is
// cross-site and Chrome gives it a target of its own to click in.
//
// Needs `dist/librepaper` (run `make build`) and chromium on PATH.

import assert from "node:assert/strict";
import { build } from "vite";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(here));
const binary = join(root, "dist", "librepaper");
assert.ok(existsSync(binary), "run `make build` first");

const temporary = mkdtempSync(join(tmpdir(), "librepaper-consent-check-"));
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
const appPort = 18700 + Math.floor(Math.random() * 200);
const pagePort = 19100 + Math.floor(Math.random() * 200);
const appAddress = `http://127.0.0.1:${appPort}/`;

writeFileSync(entry, `
import * as local from ${JSON.stringify(join(root, "web/src/lib/latex/local.js"))};
window.local = local;
window.errors = [];
addEventListener("error", (event) => window.errors.push(String(event.message)));
addEventListener("unhandledrejection", (event) => window.errors.push(String(event.reason)));
local.configure({ project: "paper-check", origin: location.origin });
local.setAddress(${JSON.stringify(appAddress)});
window.pairing = null;
window.messages = [];
addEventListener("message", (event) => window.messages.push({ origin: event.origin, data: event.data }));
window.startPairing = () => { window.pairing = local.pairViaApp().then((status) => ({ state: status.state, error: status.error })); };
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

// The local app, with its state and cache kept out of the real home.
const stateHome = join(temporary, "state");
const cacheHome = join(temporary, "cache");
const app = spawn(binary, ["local", "start", "--port", String(appPort), "--code", "123456"], {
  env: { ...process.env, XDG_STATE_HOME: stateHome, XDG_CACHE_HOME: cacheHome, HOME: temporary },
  stdio: ["ignore", "pipe", "pipe"],
});
let appLog = "";
app.stdout.on("data", (chunk) => (appLog += chunk));
app.stderr.on("data", (chunk) => (appLog += chunk));
await until("local app health", async () => {
  const response = await fetch(`${appAddress}librepaper/local/v1/health`);
  return response.ok;
}, 15000);

let b;
try {
  b = await browser("chromium", join(temporary, "chrome"), 19400 + Math.floor(Math.random() * 200));
  await b.navigate(`http://localhost:${pagePort}/`);
  await until("client loaded", () => b.evaluate("Boolean(window.local)"), 10000);
  assert.equal(await b.evaluate("window.local.bindingId()"), "hosted", "the hosted binding is the default");

  // Nothing is paired yet: the app is reachable but has not allowed this site.
  const before = await b.evaluate("window.local.retry().then((s) => s.state)");
  assert.equal(before, "unauthorized", `before consent: ${before}`);

  // The reader opens the consent page as a popup from a click; the gesture
  // is what lets the popup through, as it does for a person.
  await b.evaluateWithGesture("window.startPairing()");
  const PAIR = "/librepaper/local/v1/pair";
  await until("consent page shown", () => b.popupEvaluate(PAIR, "Boolean(document.querySelector('button.allow'))"), 10000);
  const shown = await b.popupEvaluate(PAIR, "document.body.innerText");
  assert.match(shown, /localhost:\d+/, "the page names the site asking");
  assert.match(shown, /paper-check/, "the page names the document");
  await b.popupEvaluate(PAIR, "document.querySelector('button.allow').click()");

  // Wait on the client's status rather than the pairing promise, so a
  // failure reports what the page saw instead of hanging on a promise.
  try {
    await until("pairing arrives", () => b.evaluate("window.local.status().state === 'connected'"), 15000);
  } catch (error) {
    const messages = await b.evaluate("JSON.stringify(window.messages)");
    const frame = await b.popupEvaluate(PAIR, "document.body.innerText + ' | ' + location.href").catch((e) => `popup: ${e.message}`);
    const status = await b.evaluate("JSON.stringify(window.local.status())");
    throw new Error(`${error.message}\nmessages: ${messages}\nframe: ${frame}\nstatus: ${status}\napp log: ${appLog}`);
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

  console.log("local-consent-browser: hosted binding by default, consent page names site and document, Allow pairs and connects, one pairing for the reader's origin passed");
} finally {
  if (b) await b.close();
  app.kill();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
