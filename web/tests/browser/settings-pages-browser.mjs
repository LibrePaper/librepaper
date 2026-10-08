// Where a setting lives, in the dialog itself. A tool that builds the
// document is configured on the Rendering page; the Tools page holds the
// programs on this computer and the connection to them; Diagnostics is for
// troubleshooting the companion; and the Account page carries the server.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-settings-pages-"));
const screenshotDir = process.env.SETTINGS_SCREENSHOT_DIR;
if (screenshotDir) mkdirSync(screenshotDir, { recursive: true });
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");
const statusMock = join(temporary, "status.svelte.js");
const clientMock = join(temporary, "client.js");
async function freePort() {
  const probe = createServer();
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const port = probe.address().port;
  await new Promise((resolve) => probe.close(resolve));
  return port;
}

writeFileSync(harness, `
<script>
  import SettingsDialog from ${JSON.stringify(join(root, "web/src/components/settings/SettingsDialog.svelte"))};
  let open = $state(false);
  let category = $state("rendering");
  let sourceFormat = $state("quarto");
  window.show = (page, format) => { category = page; sourceFormat = format; open = true; };
  window.applied = [];
</script>
<SettingsDialog bind:open bind:category {sourceFormat} mayEdit={true} userId="user"
                onbuildpreferences={(format, next) => { window.applied.push(next); }}
                onquartooptions={(next) => { window.applied.push(next); }}
                remoteConnected={true} />
`);

writeFileSync(statusMock, `
let current = $state.raw({ state: "connected", address: "http://127.0.0.1:8763/", capabilities: {
  tools: { quarto: { available: true, version: "1.6.0" } },
  calepin: { available: true, version: "0.3.0" },
  zotero: { available: true, version: "7.0" },
  builders: [{ id: "quarto", available: true, version: "1.6.0", outputs: ["html", "pdf"], operations: [{ kind: "build", workspace_modes: ["snapshot"] }] }],
} });
window.setCompanionState = (state) => { current = { ...current, state }; };
export const companion = {
  get status() { return current; },
  watch() { return () => {}; },
};
`);

writeFileSync(clientMock, `
window.probeCalls = 0;
export function address() { return "http://127.0.0.1:8763/"; }
export function setAddress() {}
export function probe() { window.probeCalls += 1; return Promise.resolve(); }
export function retry() { return Promise.resolve(); }
export function disconnect() { return Promise.resolve(); }
export function subscribe(listener) { listener({ state: "connected", address: "http://127.0.0.1:8763/", instance: "test" }); return () => {}; }
export function canManage() { return false; }
export function manage() { throw new Error("Management API should not be used without a manage capability."); }
export function connectApp() { return window.connectFailure ? Promise.reject(new Error(window.connectFailure)) : Promise.resolve(); }
export function chooseFolderBinding() { return Promise.resolve({ id: "folder", entrypoint: "main.qmd" }); }
export async function capabilities() { return {}; }
export async function settings() {
  return { version: "1.0.0", standalone: true, startup: false, integrations: { quarto: { path: null, args: [] }, calepin: { path: null, args: [] } } };
}
export async function setIntegration(_name, custom) { return custom; }
export async function setStartup() {}
export async function quit() {}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import Harness from ${JSON.stringify(harness)};
mount(Harness, { target: document.body });
`);

const mockModules = {
  name: "settings-pages-test-mocks",
  enforce: "pre",
  resolveId(source, importer) {
    if (source.endsWith("/lib/companion/status.svelte.js")) return statusMock;
    if (source.endsWith("/lib/companion/client.js") || (source === "./client.js" && importer?.endsWith("/lib/companion/machine.svelte.js"))) return clientMock;
    return null;
  },
};

await build({
  configFile: false,
  root: join(root, "web"),
  logLevel: "error",
  plugins: [mockModules, svelte(), tailwindcss()],
  // One file: the dialog reaches modules that import lazily, and the server
  // below serves nothing but the bundle and its stylesheet.
  build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" }, rollupOptions: { output: { codeSplitting: false } } },
});

const page = `<!doctype html><html data-theme="librepaper"><head><meta charset="utf-8"><link rel="stylesheet" href="/check.css"><title>settings pages check</title></head><body><script type="module" src="/check.js"></script></body></html>`;
const server = createServer((request, response) => {
  if (request.url === "/check.js") {
    response.setHeader("content-type", "text/javascript");
    response.end(readFileSync(join(output, "check.js")));
    return;
  }
  if (request.url === "/check.css") {
    response.setHeader("content-type", "text/css");
    response.end(readFileSync(join(output, "check.css")));
    return;
  }
  response.setHeader("content-type", "text/html");
  response.end(page);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
const debugPort = await freePort();
let b;

try {
  b = await browser("chromium", join(temporary, "chrome"), debugPort);
  await b.navigate(`http://127.0.0.1:${port}/`);
  const heading = () => b.evaluate(`document.querySelector(".settings-category")?.textContent.trim() || ""`);
  const present = (ids) => b.evaluate(`JSON.stringify(${JSON.stringify(ids)}.filter((id) => document.querySelector(".settings-body #" + id)))`).then(JSON.parse);
  const show = async (pageId, format, says) => {
    await b.evaluate(`window.show(${JSON.stringify(pageId)}, ${JSON.stringify(format)})`);
    await until(`${says} page for ${format}`, () => heading().then((text) => text === says), 5000);
  };
  const subheads = () => b.evaluate(`JSON.stringify([...document.querySelectorAll(".settings-body .settings-subhead")].map((node) => node.textContent.trim()))`).then(JSON.parse);
  const capture = async (name) => {
    if (!screenshotDir) return;
    for (const [size, width] of [["desktop", 1280], ["mobile", 390]]) {
      await b.resize(width, 900);
      await b.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
      const shot = await b.command("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(screenshotDir, `${name}-${size}.png`), Buffer.from(shot.data, "base64"));
      assert.equal(await b.evaluate("document.querySelector('.settings-body').scrollWidth <= document.querySelector('.settings-body').clientWidth + 1"), true, `${name} fits ${size} width`);
    }
    await b.resize(1280, 900);
  };
  const everything = ["render-latex-engine", "render-latex-files", "render-markdown-tool", "rendering-profile", "rendering-parameters",
    "tools-list", "tools-quarto", "tools-calepin", "tools-zotero", "quarto-executable", "quarto-arguments",
    "diagnostics-address", "diagnostics-startup", "diagnostics-report",
    "remote-status", "remote-address", "storage-account", "account-erase"];
  const renderingRows = ["render-latex-engine", "render-latex-files", "render-markdown-tool", "rendering-profile", "rendering-parameters"];
  const renderingSections = ["LaTeX", "Markdown and Quarto"];

  // The navigation, in order, for a visitor on a Quarto document.
  await until("harness mounted", () => b.evaluate("typeof window.show === \"function\""), 10000);
  await show("rendering", "quarto", "Rendering");
  assert.deepEqual(JSON.parse(await b.evaluate(`JSON.stringify([...document.querySelectorAll(".settings-nav-item")].map((node) => node.textContent.trim()))`)),
    ["Editor", "Rendering", "Tools", "AI agents", "Backups", "Account", "Diagnostics"]);

  // Rendering shows two sections with global build options visible for all formats.
  await until("the Quarto profile rows", () => present(["rendering-profile"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), renderingRows);
  assert.deepEqual(await subheads(), renderingSections);
  assert.equal(await b.evaluate(`document.querySelector(".settings-group-title") === null`), true);
  // The companion and the programs it finds live on the Tools page, not here.
  assert.equal(await b.evaluate(`[...document.querySelectorAll(".settings-body .setting-status-pill")].some((pill) => /Companion|Running|Not running|Needs approval|Blocked|Update needed/.test(pill.textContent))`), false, "Rendering has no companion or program status pill");
  assert.equal(await b.evaluate(`document.querySelector("#quarto-executable") === null`), true, "Rendering has no executable fields");
  await capture("rendering-connected");
  if (screenshotDir) {
    await b.evaluate("document.querySelector('.settings-body').scrollTop = document.querySelector('.settings-body').scrollHeight");
    await capture("rendering-bottom");
    await b.evaluate("document.querySelector('.settings-body').scrollTop = 0");
  }
  // Quarto profile and parameters are stored globally, not per document.
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').value`), "");
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').value`), "");

  // The same sections remain visible for all formats.
  await show("rendering", "typst", "Rendering");
  assert.deepEqual(await present(everything), renderingRows);
  assert.deepEqual(await subheads(), renderingSections);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').disabled`), false);
  await show("rendering", "latex", "Rendering");
  await until("the LaTeX rows", () => present(["render-latex-files"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), renderingRows);
  assert.deepEqual(await subheads(), renderingSections);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').disabled`), false);
  await show("rendering", "html", "Rendering");
  assert.deepEqual(await subheads(), renderingSections);
  assert.ok((await present(everything)).includes("render-latex-files"));
  // None of that looked for the companion: the Rendering page must not make the
  // browser ask for local network access just by opening.
  assert.equal(await b.evaluate("window.probeCalls"), 0, "the Rendering page does not probe for the companion");

  // The Tools page is the connection and the programs on this computer. Zotero
  // feeds a document without building it, and has no command of its own.
  await show("tools", "quarto", "Tools");
  await until("the program rows", () => present(["tools-zotero"]).then((found) => found.length === 1), 5000);
  await capture("tools");
  assert.deepEqual(await present(everything), ["tools-list", "tools-quarto", "tools-calepin", "tools-zotero"]);
  assert.equal(await b.evaluate('Boolean(document.querySelector("#settings-companion"))'), true, "the Companion status sits in the dialog on the Tools page");
  assert.deepEqual(await subheads(), ["Programs"], "Connected sites needs a managing connection");
  assert.match(await b.evaluate(`document.querySelector("#tools-zotero .setting-status-pill")?.textContent`), /Available · 7\.0/);
  assert.equal(await b.evaluate(`document.querySelector("#tools-zotero .tool-row-toggle")`), null, "Zotero has no command to edit");
  assert.match(await b.evaluate(`document.querySelector("#settings-companion .setting-status-pill")?.textContent`), /Running/);
  // Quarto lists its executable and arguments behind Details.
  assert.equal(await b.evaluate(`document.querySelector("#quarto-executable") === null`), true, "the executable row starts hidden");
  await b.evaluate(`document.querySelector("#tools-quarto .tool-row-toggle").click()`);
  await until("the Quarto command rows", () => present(["quarto-executable", "quarto-arguments"]).then((found) => found.length === 2), 5000);
  assert.deepEqual(await present(everything), ["tools-list", "tools-quarto", "tools-calepin", "tools-zotero", "quarto-executable", "quarto-arguments"]);

  // The Diagnostics page is the address, the machine and the setup check.
  await show("diagnostics", "quarto", "Diagnostics");
  await until("the startup row", () => present(["diagnostics-startup"]).then((found) => found.length === 1), 5000);
  await capture("diagnostics");
  assert.deepEqual(await present(everything), ["diagnostics-address", "diagnostics-startup", "diagnostics-report"]);

  // A visitor's Account page is the server; storage and erasure need an account.
  await show("account", "quarto", "Account");
  await capture("account");
  assert.deepEqual(await present(everything), ["remote-status", "remote-address"]);
  assert.match(await b.evaluate(`document.querySelector("#remote-status .setting-status-pill")?.textContent`), /Connected/);
  assert.match(await b.evaluate(`document.querySelector("#remote-status").textContent`), /Remote connection/);

  // Offline settings stay visible. Build tool selections are available globally,
  // while the command fields for local tools are disabled.
  await b.evaluate(`window.setCompanionState("unreachable")`);
  await show("rendering", "quarto", "Rendering");
  await capture("rendering-offline");
  assert.deepEqual(await subheads(), renderingSections);
  await until("offline rendering settings", () => present(["render-latex-engine"]).then((found) => found.length === 1), 5000);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="LaTeX engine"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').disabled`), false);
  await show("tools", "quarto", "Tools");
  assert.equal(await b.evaluate(`document.querySelector("#tools-calepin .setting-status-pill")?.textContent.trim()`), "Not checked");
  assert.equal(await b.evaluate(`document.querySelector("#tools-zotero .setting-status-pill")?.textContent.trim()`), "Not checked");
  assert.match(await b.evaluate(`document.querySelector("#settings-companion .setting-status-pill")?.textContent`), /Not running/);
  assert.match(await b.evaluate(`document.querySelector("#settings-companion").textContent`), /Install|Connect/);
  // A failed Connect lands on its own line under the status pill instead of
  // running beside it.
  await b.resize(1280, 900);
  await b.evaluate(`window.connectFailure = "LibrePaper Companion is not running on this computer. Start it, then connect again."`);
  await b.evaluate(`[...document.querySelectorAll("#settings-companion button")].find((button) => button.textContent.trim() === "Connect").click()`);
  await until("the Connect failure", () => b.evaluate(`Boolean(document.querySelector("#settings-companion [role=alert]"))`), 5000);
  const placed = JSON.parse(await b.evaluate(`(() => {
    const pill = document.querySelector("#settings-companion .setting-status-pill").getBoundingClientRect();
    const failure = document.querySelector("#settings-companion [role=alert]").getBoundingClientRect();
    return JSON.stringify({ pillBottom: pill.bottom, failureTop: failure.top });
  })()`));
  assert.ok(placed.failureTop >= placed.pillBottom - 1, "the failure sits below the status pill");
  assert.notEqual(await b.evaluate(`getComputedStyle(document.querySelector("#settings-companion [role=alert]")).color`),
    await b.evaluate(`getComputedStyle(document.querySelector("#settings-companion .companion-status-label")).color`), "the failure keeps its error colour, not the muted label colour");
  assert.equal(await b.evaluate(`document.querySelector("#tools-zotero .tool-row-toggle")`), null, "Zotero does not load an editable companion config");
  await b.evaluate(`document.querySelector("#tools-quarto .tool-row-toggle").click()`);
  await until("offline Quarto command rows", () => present(["quarto-executable"]).then((found) => found.length === 1), 5000);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Executable path"]')?.disabled`), true);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Arguments"]')?.disabled`), true);

  await capture("tools-offline");
  await show("diagnostics", "quarto", "Diagnostics");
  await capture("diagnostics-offline");
  await show("backups", "quarto", "Backups");
  await capture("backups-offline");

  console.log("settings-pages-browser: Rendering shows two sections with global format options for all documents and no companion rows, Tools lists the programs with Quarto commands behind Details, Diagnostics holds the address and setup check, Quarto settings remain editable offline, and local command fields disable without the companion");
} finally {
  if (b) await b.close();
  server.close();
  removeTemporary(temporary);
}
