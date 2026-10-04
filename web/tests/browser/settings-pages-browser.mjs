// Where a setting lives, in the dialog itself. A tool that builds the
// document is configured on the Render page, whole; Integrations holds what
// feeds a document without building it; the Companion page manages this
// computer; and the Account page carries the server for everybody.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

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
const controlMock = join(temporary, "control.js");
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
  let category = $state("render");
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
export function connectApp() { return Promise.resolve(); }
export function chooseFolderBinding() { return Promise.resolve({ id: "folder", entrypoint: "main.qmd" }); }
export async function capabilities() { return {}; }
export async function settings() {
  return { version: "1.0.0", standalone: true, startup: false, integrations: { quarto: { path: null, args: [] }, calepin: { path: null, args: [] } } };
}
export async function setIntegration(_name, custom) { return custom; }
export async function setStartup() {}
export async function quit() {}
`);

writeFileSync(controlMock, `
export function available() { return false; }
export function scope() { return ""; }
export function subscribe(listener) { listener({ available: false, scope: "" }); return () => {}; }
export function request() { throw new Error("Management API should not be used without a credential."); }
export function openSettings() {}
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
  resolveId(source) {
    if (source.endsWith("/lib/companion/status.svelte.js")) return statusMock;
    if (source.endsWith("/lib/companion/client.js")) return clientMock;
    if (source.endsWith("/lib/companion/control.js")) return controlMock;
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
  const everything = ["render-local", "render-latex-engine", "render-latex-files", "calepin-status", "calepin-executable", "calepin-arguments",
    "render-markdown-tool", "quarto-status", "quarto-executable", "quarto-arguments",
    "rendering-profile", "rendering-parameters", "integrations-companion", "zotero-status",
    "local-status", "local-address", "local-doctor", "remote-status", "remote-address", "storage-account", "account-erase"];

  // The navigation, in order, for a visitor on a Quarto document.
  await until("harness mounted", () => b.evaluate("typeof window.show === \"function\""), 10000);
  await show("render", "quarto", "Render");
  assert.deepEqual(JSON.parse(await b.evaluate(`JSON.stringify([...document.querySelectorAll(".settings-nav-item")].map((node) => node.textContent.trim()))`)),
    ["Editor", "Render", "Integrations", "Companion", "Backups", "Account"]);

  // Render always shows four sections with global build options visible for all formats.
  await until("the Quarto rows", () => present(["quarto-executable"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), ["render-local", "render-latex-engine", "render-latex-files", "calepin-status", "calepin-executable", "calepin-arguments",
    "render-markdown-tool", "quarto-status", "quarto-executable", "quarto-arguments",
    "rendering-profile", "rendering-parameters"]);
  assert.deepEqual(await subheads(), ["Detected tools", "LaTeX", "Typst and Calepin", "Markdown and Quarto"]);
  assert.equal(await b.evaluate(`document.querySelector(".settings-group-title") === null`), true);
  await capture("render-connected");
  if (screenshotDir) {
    await b.evaluate("document.querySelector('.settings-body').scrollTop = document.querySelector('.settings-body').scrollHeight");
    await capture("render-tools");
    await b.evaluate("document.querySelector('.settings-body').scrollTop = 0");
  }
  // Quarto profile and parameters are stored globally, not per document.
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').value`), "");
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').value`), "");

  // The same sections remain visible for all formats.
  await show("render", "typst", "Render");
  await until("the Calepin rows", () => present(["calepin-executable"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), ["render-local", "render-latex-engine", "render-latex-files", "calepin-status", "calepin-executable", "calepin-arguments",
    "render-markdown-tool", "quarto-status", "quarto-executable", "quarto-arguments", "rendering-profile", "rendering-parameters"]);
  assert.deepEqual(await subheads(), ["Detected tools", "LaTeX", "Typst and Calepin", "Markdown and Quarto"]);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').disabled`), false);
  await show("render", "latex", "Render");
  await until("the LaTeX rows", () => present(["render-latex-files"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), ["render-local", "render-latex-engine", "render-latex-files", "calepin-status", "calepin-executable", "calepin-arguments",
    "render-markdown-tool", "quarto-status", "quarto-executable", "quarto-arguments", "rendering-profile", "rendering-parameters"]);
  assert.deepEqual(await subheads(), ["Detected tools", "LaTeX", "Typst and Calepin", "Markdown and Quarto"]);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').disabled`), false);
  await show("render", "html", "Render");
  assert.deepEqual(await subheads(), ["Detected tools", "LaTeX", "Typst and Calepin", "Markdown and Quarto"]);
  assert.ok((await present(everything)).includes("render-latex-files"));
  // None of that looked for the companion: the Render page must not make the
  // browser ask for local network access just by opening.
  assert.equal(await b.evaluate("window.probeCalls"), 0, "the Render page does not probe for the companion");

  // Zotero feeds a document without building it.
  await show("integrations", "quarto", "Integrations");
  await capture("zotero");
  assert.deepEqual(await present(everything), ["integrations-companion", "zotero-status"]);
  assert.deepEqual(await subheads(), ["Zotero"]);

  // The Companion page is the connection and the machine, not the tools.
  await show("local", "quarto", "Companion");
  await capture("companion");
  assert.deepEqual(await present(everything), ["local-status", "local-address", "local-doctor"]);

  // A visitor's Account page is the server; storage and erasure need an account.
  await show("account", "quarto", "Account");
  await capture("account");
  assert.deepEqual(await present(everything), ["remote-status", "remote-address"]);
  assert.match(await b.evaluate(`document.querySelector("#remote-status .setting-status-pill")?.textContent`), /Connected/);
  assert.match(await b.evaluate(`document.querySelector("#remote-status").textContent`), /Remote connection/);

  // Offline settings stay visible. Build tool selections are available globally,
  // while integration settings for local tools are disabled.
  await b.evaluate(`window.setCompanionState("unreachable")`);
  await show("render", "quarto", "Render");
  await capture("render-offline");
  assert.deepEqual(await subheads(), ["Detected tools", "LaTeX", "Typst and Calepin", "Markdown and Quarto"]);
  await until("offline render settings", () => present(["render-latex-engine"]).then((found) => found.length === 1), 5000);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="LaTeX engine"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector("#calepin-status .setting-status-pill")?.textContent.trim()`), "Not checked");
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Executable path"]')?.disabled`), true);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Arguments"]')?.disabled`), true);
  await show("integrations", "zotero", "Integrations");
  assert.match(await b.evaluate(`document.querySelector('.integration-note').textContent`), /local API/i);
  assert.match(await b.evaluate(`document.querySelector('.integration-note').textContent`), /Settings/i);
  assert.match(await b.evaluate(`document.querySelector('.settings-body').textContent`), /Companion|Not running/i);
  assert.equal(await b.evaluate(`document.querySelector('.integration-error') !== null`), false, "Zotero does not load an editable companion config");

  await capture("zotero-offline");
  await show("local", "quarto", "Companion");
  await capture("companion-offline");
  await show("backups", "quarto", "Backups");
  await capture("backups-offline");

  console.log("settings-pages-browser: Render shows four sections with global format options visible for all documents, Quarto settings remain editable offline, local integration controls disable without the companion, and Remote and Zotero explain their status");
} finally {
  if (b) await b.close();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
