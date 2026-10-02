// Where a setting lives, in the dialog itself. A tool that builds the
// document is configured on the Render page, whole; Integrations holds what
// feeds a document without building it; the Companion page is the connection
// and nothing else; and the Account page carries the server for everybody.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../../tools/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-settings-pages-"));
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
  let category = $state("render");
  let sourceFormat = $state("quarto");
  let buildPreferences = $state({ selection: "tool", backend: "local", tool: "quarto", profile: "draft", parameters: { year: 2026 } });
  window.show = (page, format) => { category = page; sourceFormat = format; open = true; };
  window.applied = [];
</script>
<SettingsDialog bind:open bind:category {sourceFormat} mayEdit={true} main="main.qmd"
                {buildPreferences} documentId="doc" userId="user"
                onapplyoptions={(next) => { window.applied.push(next); buildPreferences = { ...buildPreferences, ...next }; }}
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

const page = `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="/check.css"><title>settings pages check</title></head><body><script type="module" src="/check.js"></script></body></html>`;
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
  const everything = ["render-tool", "render-folder", "render-output", "render-latex-files", "typst-status", "quarto-status", "quarto-executable", "quarto-arguments",
    "rendering-profile", "rendering-parameters", "quarto-execution", "calepin-status", "calepin-executable", "calepin-arguments", "zotero-status",
    "local-status", "local-address", "local-doctor", "remote-status", "remote-address", "storage-account", "account-erase"];

  // The navigation, in order, for a visitor on a Quarto document.
  await until("harness mounted", () => b.evaluate("typeof window.show === \"function\""), 10000);
  await show("render", "quarto", "Render");
  assert.deepEqual(JSON.parse(await b.evaluate(`JSON.stringify([...document.querySelectorAll(".settings-nav-item")].map((node) => node.textContent.trim()))`)),
    ["Editor", "Render", "Integrations", "Companion", "Backups", "Account"]);

  // Render always shows its four sections. Quarto settings remain together
  // with the browser's build choices and work even without a companion.
  await until("the Quarto rows", () => present(["quarto-executable"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), ["render-tool", "render-folder", "render-output", "render-latex-files", "typst-status", "quarto-status", "quarto-executable", "quarto-arguments",
    "rendering-profile", "rendering-parameters", "quarto-execution", "calepin-status", "calepin-executable", "calepin-arguments"]);
  assert.deepEqual(await subheads(), ["LaTeX", "Typst", "Quarto", "Calepin"]);
  // The profile and parameters come from the build preferences, so what was
  // stored for this document is what the page opens on.
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').value`), "draft");
  assert.match(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').value`), /"year": 2026/);
  await b.evaluate(`(() => { const input = document.querySelector('[aria-label="Quarto profile"]'); input.value = "final"; input.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  await b.evaluate(`[...document.querySelectorAll(".settings-body button")].find((node) => node.textContent.trim() === "Apply").click()`);
  await until("options applied", () => b.evaluate("window.applied.length === 1"), 5000);
  assert.deepEqual(JSON.parse(await b.evaluate("JSON.stringify(window.applied[0])")).profile, "final");

  // The same sections remain visible for Typst and LaTeX. Quarto preferences
  // stay visible but are disabled where they cannot affect the preview.
  await show("render", "typst", "Render");
  await until("the Calepin rows", () => present(["calepin-executable"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), ["render-tool", "render-folder", "render-output", "render-latex-files", "typst-status", "quarto-status", "quarto-executable", "quarto-arguments",
    "rendering-profile", "rendering-parameters", "quarto-execution", "calepin-status", "calepin-executable", "calepin-arguments"]);
  assert.deepEqual(await subheads(), ["LaTeX", "Typst", "Quarto", "Calepin"]);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').disabled`), true);
  await show("render", "latex", "Render");
  await until("the LaTeX rows", () => present(["render-latex-files"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), ["render-tool", "render-latex-files", "typst-status", "quarto-status", "quarto-executable", "quarto-arguments",
    "rendering-profile", "rendering-parameters", "quarto-execution", "calepin-status", "calepin-executable", "calepin-arguments"]);
  assert.deepEqual(await subheads(), ["LaTeX", "Typst", "Quarto", "Calepin"]);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').disabled`), true);
  await show("render", "html", "Render");
  assert.deepEqual(await subheads(), ["LaTeX", "Typst", "Quarto", "Calepin"]);
  assert.ok((await present(everything)).includes("render-latex-files"));
  // None of that looked for the companion: the Render page must not make the
  // browser ask for local network access just by opening.
  assert.equal(await b.evaluate("window.probeCalls"), 0, "the Render page does not probe for the companion");

  // Zotero feeds a document without building it.
  await show("integrations", "quarto", "Integrations");
  assert.deepEqual(await present(everything), ["zotero-status"]);
  assert.deepEqual(await subheads(), ["Zotero"]);

  // The Companion page is the connection and the machine, not the tools.
  await show("local", "quarto", "Companion");
  assert.deepEqual(await present(everything), ["local-status", "local-address", "local-doctor"]);

  // A visitor's Account page is the server; storage and erasure need an account.
  await show("account", "quarto", "Account");
  assert.deepEqual(await present(everything), ["remote-status", "remote-address"]);
  assert.match(await b.evaluate(`document.querySelector('[aria-label^="Remote connection"]').textContent`), /Connected/);
  assert.match(await b.evaluate(`document.querySelector("#remote-status").textContent`), /Remote connection/);

  // Offline settings stay visible while controls that need the companion are
  // unavailable. Quarto's browser preferences stay editable offline, while
  // local build output and integration settings are visibly disabled.
  await b.evaluate(`window.setCompanionState("unreachable")`);
  await show("render", "quarto", "Render");
  assert.deepEqual(await subheads(), ["LaTeX", "Typst", "Quarto", "Calepin"]);
  await until("offline folder and output rows", () => present(["render-folder", "render-output"]).then((found) => found.length === 2), 5000);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Project entrypoint"]').disabled`), true);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Build output"]').disabled`), true);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Executable path"]').disabled`), true);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Arguments"]').disabled`), true);
  await show("integrations", "zotero", "Integrations");
  assert.match(await b.evaluate(`document.querySelector('.integration-note').textContent`), /enable its local API/i);
  assert.match(await b.evaluate(`document.querySelector('.settings-body').textContent`), /Connect the LibrePaper Companion/);
  assert.equal(await b.evaluate(`document.querySelector('.integration-error') !== null`), false, "Zotero does not load an editable companion config");

  console.log("settings-pages-browser: Render keeps its four sections visible for all formats, browser Quarto options remain editable offline, local controls disable without the companion, and Remote and Zotero explain their status");
} finally {
  if (b) await b.close();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
