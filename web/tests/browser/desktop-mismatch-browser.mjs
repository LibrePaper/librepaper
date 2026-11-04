// The version check between a page and the desktop app, end to end: the real
// client.js and negotiate.js decide, and the real banner and feature wrapper
// show the answer. A page global is set before the page's scripts run, the way
// the desktop app sets it. The shell's update check is answered by this test's
// own HTTP server.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { paraglideVitePlugin } from "@inlang/paraglide-js";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-desktop-mismatch-"));
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");

async function freePort() {
  const probe = createServer();
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const port = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));
  return port;
}

writeFileSync(harness, `
<script>
  import DesktopMismatch from ${JSON.stringify(join(root, "web/src/components/DesktopMismatch.svelte"))};
  import DesktopOnly from ${JSON.stringify(join(root, "web/src/components/DesktopOnly.svelte"))};
</script>
<DesktopMismatch />
<DesktopOnly feature="Shared links">
  <span id="desktop-child">Desktop child</span>
</DesktopOnly>
<p id="harness-ready">ready</p>
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import Harness from ${JSON.stringify(harness)};
mount(Harness, { target: document.body });
`);

await build({
  configFile: false,
  root: join(root, "web"),
  logLevel: "error",
  // The real client imports the compiled messages; compile them the same way
  // web/vite.config.js does.
  plugins: [
    paraglideVitePlugin({ project: join(root, "web/project.inlang"), outdir: join(root, "web/src/lib/paraglide") }),
    svelte(),
    tailwindcss(),
  ],
  build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" } },
});

// The page global the desktop app would set, or none in a plain browser.
const shellGlobal = (name, base) => ({
  plain: null,
  current: { base, version: "0.0.70", protocol: { oldest: 3, current: "3.1.0" } },
  legacy: { base, version: "0.0.70" },
  "server-old": { base, version: "0.0.99", protocol: { oldest: 4, current: "4.0.0" } },
  "app-old": { base, version: "0.0.60", protocol: { oldest: 1, current: "2.0.0" } },
})[name];

const pageFor = (shell) => `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="/check.css"><title>desktop mismatch check</title>${shell ? `<script>window.__LIBREPAPER_DESKTOP__ = ${JSON.stringify(shell)}</script>` : ""}</head><body><script type="module" src="/check.js"></script></body></html>`;

const shellCalls = { updates: 0 };
let port;
const server = createServer((request, response) => {
  const path = new URL(request.url, "http://127.0.0.1").pathname;
  if (path === "/check.js") {
    response.setHeader("content-type", "text/javascript");
    response.end(readFileSync(join(output, "check.js")));
    return;
  }
  if (path === "/check.css") {
    response.setHeader("content-type", "text/css");
    response.end(readFileSync(join(output, "check.css")));
    return;
  }
  if (path === "/desktop/shell/check-update" && request.method === "POST") {
    shellCalls.updates += 1;
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ via: "package_manager" }));
    return;
  }
  const match = path.match(/^\/case\/([a-z-]+)$/);
  if (match && ["plain", "current", "legacy", "server-old", "app-old"].includes(match[1])) {
    response.setHeader("content-type", "text/html");
    response.end(pageFor(shellGlobal(match[1], `http://127.0.0.1:${port}/desktop/`)));
    return;
  }
  response.statusCode = 404;
  response.end();
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
port = server.address().port;
const debugPort = await freePort();
let b;

// Each case is a fresh page load; the harness marks itself ready after mount.
async function open(name) {
  await b.navigate(`http://127.0.0.1:${port}/case/${name}`);
  await until(`${name} mounted`, () => b.evaluate("Boolean(document.querySelector('#harness-ready'))"), 10000);
  return JSON.parse(await b.evaluate(`JSON.stringify({
    mismatch: Boolean(document.querySelector('.desktop-mismatch')),
    mismatchRole: document.querySelector('.desktop-mismatch')?.getAttribute('role') ?? null,
    mismatchText: document.querySelector('.desktop-mismatch')?.innerText.trim() ?? null,
    mismatchButton: document.querySelector('.desktop-mismatch button')?.textContent.trim() ?? null,
    notice: document.querySelector('.desktop-only')?.innerText.trim() ?? null,
    noticeMessage: document.querySelector('.desktop-only .desktop-only-message')?.textContent.trim() ?? null,
    child: document.querySelector('#desktop-child')?.textContent.trim() ?? null,
  })`));
}

try {
  b = await browser("chromium", join(temporary, "chrome"), debugPort);

  // 1. A plain browser: no banner, and the wrapper shows only its install notice.
  const plain = await open("plain");
  assert.equal(plain.mismatch, false, "a plain browser shows no mismatch banner");
  assert.equal(plain.child, null, "a plain browser does not render the desktop child");
  assert.notEqual(plain.notice, null, "a plain browser shows the desktop-only notice");

  // 2. A same-protocol app: no banner, the child renders.
  const current = await open("current");
  assert.equal(current.mismatch, false, "agreeing versions show no banner");
  assert.equal(current.child, "Desktop child", "the desktop child renders when the app agrees");
  assert.equal(current.notice, null, "no desktop-only notice inside the app");

  // 3. An app that predates the protocol field is taken to speak the legacy range.
  const legacy = await open("legacy");
  assert.equal(legacy.mismatch, false, "a legacy app shows no banner");
  assert.equal(legacy.child, "Desktop child", "the child renders for a legacy app");

  // 4. The server is older than the app's oldest supported protocol: the page
  // says the server needs an administrator, with no button.
  const serverOld = await open("server-old");
  assert.equal(serverOld.mismatch, true, "the mismatch banner shows");
  assert.equal(serverOld.mismatchRole, "status", "the banner is a status region");
  assert.match(serverOld.mismatchText, new RegExp(`127\\.0\\.0\\.1:${port}`), "the banner names the page's host");
  assert.match(serverOld.mismatchText, /administrator/, "the banner asks an administrator to upgrade");
  assert.equal(serverOld.mismatchButton, null, "a server mismatch offers no update button");
  assert.equal(serverOld.child, null, "the desktop child stays off on a mismatch");
  assert.equal(serverOld.noticeMessage, serverOld.mismatchText, "the desktop-only notice gives the same reason");

  // 5. The app is too old for this page: the banner names the app's version and
  // offers an update, which the shell answers with the package manager.
  const appOld = await open("app-old");
  assert.equal(appOld.mismatch, true, "the mismatch banner shows for an old app");
  assert.match(appOld.mismatchText, /0\.0\.60/, "the banner names the app's version");
  assert.equal(appOld.mismatchButton, "Update now", "an old app offers the update button");
  assert.equal(appOld.child, null, "the desktop child stays off for an old app");
  await b.evaluate("document.querySelector('.desktop-mismatch button').click(), true");
  await until("package manager answer shown", () => b.evaluate("document.querySelector('.desktop-mismatch')?.innerText.includes('package manager')"), 10000);
  const answered = JSON.parse(await b.evaluate(`JSON.stringify({
    button: document.querySelector('.desktop-mismatch button')?.textContent.trim() ?? null,
    text: document.querySelector('.desktop-mismatch')?.innerText.trim() ?? null,
  })`));
  assert.equal(answered.button, null, "the update button is gone after the check");
  assert.match(answered.text, /package manager/, "the banner says to use the package manager");
  assert.equal(shellCalls.updates, 1, "the page asked the shell for an update exactly once");

  console.log("desktop-mismatch-browser: plain, agreeing, legacy, server and app mismatch cases passed");
} finally {
  if (b) await b.close();
  server.close();
  removeTemporary(temporary);
}
