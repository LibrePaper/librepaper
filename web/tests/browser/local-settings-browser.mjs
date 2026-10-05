// The local companion settings are an important permission boundary: their
// controls describe the machine LibrePaper can use, and a custom Quarto
// command is sent to the companion, which confirms it on this computer.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-local-settings-"));
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
  import DiagnosticsSettings from ${JSON.stringify(join(root, "web/src/components/settings/DiagnosticsSettings.svelte"))};
  import ToolCommand from ${JSON.stringify(join(root, "web/src/components/settings/ToolCommand.svelte"))};
  import { createMachineView } from ${JSON.stringify(join(root, "web/src/lib/companion/machine.svelte.js"))};
  const view = createMachineView();
  $effect(() => view.start());
</script>
<DiagnosticsSettings {view} />
<section id="quarto-section"><ToolCommand name="quarto" /></section>
`);

writeFileSync(statusMock, `
let current = $state.raw({ state: "unreachable", address: "http://127.0.0.1:8763/", capabilities: null });
const listeners = new Set();
export const companion = {
  get status() { return current; },
  watch() { listeners.add(true); return () => listeners.clear(); },
};
window.setLocalStatus = (next) => { current = next; };
`);

writeFileSync(clientMock, `
let currentAddress = "http://127.0.0.1:8763/";
export function address() { return currentAddress; }
export function setAddress(value) { currentAddress = value; window.savedAddress = value; }
export function probe() { return Promise.resolve(); }
export function retry() { window.retryCalls = (window.retryCalls || 0) + 1; return Promise.resolve(); }
export function disconnect() { return Promise.resolve(); }
export function connectApp() { window.pairCalls = (window.pairCalls || 0) + 1; return Promise.reject(new Error("The permission window was blocked.")); }
export function chooseFolderBinding() { return Promise.resolve({ id: "folder", entrypoint: "main.qmd" }); }
export async function capabilities(options) {
  window.capabilityCalls = (window.capabilityCalls || 0) + 1;
  window.capabilityOptions = options;
  return { tools: { quarto: { available: true, version: "1.6.0" } }, confinement: { kind: "none" } };
}
export async function settings() {
  window.settingsCalls = (window.settingsCalls || 0) + 1;
  return { version: "1.0.0", standalone: true, startup: false, integrations: { quarto: { path: null, args: [] }, calepin: { path: null, args: [] } } };
}
export async function setIntegration(name, custom) {
  window.integrationCalls = [...(window.integrationCalls || []), { name, ...custom }];
  return custom;
}
export async function setStartup() {}
export async function quit() {}
`);

writeFileSync(controlMock, `
export function available() { return false; }
export function scope() { return ""; }
export function subscribe(listener) { listener({ available: false, scope: "" }); return () => {}; }
export function request() { throw new Error("Management API should not be used without a credential."); }
export function connect() { return Promise.reject(new Error("Management API should not be used without a credential.")); }
export function showSettings() {}
export function openSettings() {}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import Harness from ${JSON.stringify(harness)};
mount(Harness, { target: document.body });
`);

const mockModules = {
  name: "local-settings-test-mocks",
  enforce: "pre",
  resolveId(source, importer) {
    if (source.endsWith("/lib/companion/status.svelte.js")) return statusMock;
    if (source.endsWith("/lib/companion/client.js") || (source === "./client.js" && importer?.endsWith("/lib/companion/machine.svelte.js"))) return clientMock;
    if (source.endsWith("/lib/companion/control.js") || (source === "./control.js" && importer?.endsWith("/lib/companion/machine.svelte.js"))) return controlMock;
    return null;
  },
};

await build({
  configFile: false,
  root: join(root, "web"),
  logLevel: "error",
  plugins: [mockModules, svelte(), tailwindcss()],
  build: { outDir: output, emptyOutDir: true, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" } },
});

const page = `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="/check.css"><title>local settings check</title></head><body><script type="module" src="/check.js"></script></body></html>`;
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
const settle = () => new Promise((resolve) => setTimeout(resolve, 80));
const clickText = (text) => `(() => { const node = [...document.querySelectorAll('button,summary,[role="switch"]')].find((item) => item.textContent.trim() === ${JSON.stringify(text)} || item.getAttribute('aria-label') === ${JSON.stringify(text)}); if (!node) throw new Error('Could not find control: ' + ${JSON.stringify(text)}); node.click(); return true; })()`;
const clickSelector = (selector) => `(() => { const node = document.querySelector(${JSON.stringify(selector)}); if (!node) throw new Error('Could not find control: ' + ${JSON.stringify(selector)}); node.click(); return true; })()`;

try {
  b = await browser("chromium", join(temporary, "chrome"), debugPort);
  await b.navigate(`http://127.0.0.1:${port}/`);
  await until("local settings mounted", () => b.evaluate("Boolean(document.querySelector('#diagnostics-connection'))"), 10000);

  // An unreachable companion links to the install page instead of carrying
  // commands. Connection details are expanded when needed.
  const initial = await b.evaluate(`JSON.stringify({
    text: document.body.innerText,
    details: document.querySelectorAll('details').length,
    summaries: document.querySelectorAll('summary').length,
    install: document.querySelector('#diagnostics-connection a[href*="install"]')?.href,
  })`);
  const disconnected = JSON.parse(initial);
  assert.equal(disconnected.install, "https://librepaper.org/install.html", "the install section links to the one install page");
  assert.doesNotMatch(disconnected.text, /curl|librepaper-installer/, "the settings page carries no install command of its own");
  assert.equal(disconnected.details, 0, "initial view has no collapsed details");
  assert.equal(disconnected.summaries, 0, "initial view has no disclosure summary");
  assert.doesNotMatch(disconnected.text, /Build presets|Current document/);
  assert.equal(await b.evaluate("Boolean(document.querySelector('#diagnostics-startup'))"), false, "startup setting is hidden while disconnected");
  assert.equal(await b.evaluate("Boolean([...document.querySelectorAll('button')].find((button) => button.textContent.trim() === 'Quit companion'))"), false, "quit companion button is hidden while disconnected");
  assert.equal(await b.evaluate("Boolean([...document.querySelectorAll('#diagnostics-address button')].find((button) => button.textContent.trim() === 'Disconnect')?.disabled)"), true, "disconnect stays visible but unavailable while disconnected");
  assert.equal(await b.evaluate("document.querySelector('#diagnostics-report button')?.disabled"), true, "setup check is unavailable while disconnected");
  assert.equal(await b.evaluate("Boolean(document.querySelector('#quarto-executable input')?.disabled && document.querySelector('#quarto-arguments input')?.disabled)"), true, "Quarto fields remain unavailable until connected");

  // A failed connection request is shown beside Connect, explaining why it happened.
  await b.evaluate("window.setLocalStatus({ state: 'unauthorized', address: 'http://127.0.0.1:8763/', capabilities: null })");
  await settle();
  await b.evaluate(clickText("Connect"));
  await until("popup failure shown", () => b.evaluate("document.body.innerText.includes('permission window was blocked')"), 5000);

  // Once connected, the install link gives way to machine controls, and
  // the Quarto section offers its own command.
  await b.evaluate(`window.setLocalStatus({ state: 'connected', address: 'http://127.0.0.1:8763/', capabilities: { tools: { quarto: { available: true, version: '1.6.0' } }, confinement: { kind: 'none' } } })`);
  await until("connected settings shown", () => b.evaluate("Boolean(document.querySelector('#quarto-executable')) && document.querySelector('#diagnostics-startup [role=switch]')?.disabled === false"), 5000);
  const connectedText = await b.evaluate("document.body.innerText");
  assert.equal(await b.evaluate("document.querySelector('#diagnostics-connection a[href*=\"install\"]')"), null, "a connected companion needs no install link");
  assert.doesNotMatch(connectedText, /permission window was blocked/);
  assert.doesNotMatch(connectedText, /Build presets|Available tools/);
  assert.match(connectedText, /Quit companion/);

  // A custom command is split into arguments, quotes kept together.
  const type = (label, value) => `(() => { const input = document.querySelector('#quarto-section [aria-label="${label}"]'); input.value = ${JSON.stringify(value)}; input.dispatchEvent(new Event('input', { bubbles: true })); })()`;
  await b.evaluate(type("Executable path", "/opt/quarto/bin"));
  await b.evaluate(type("Arguments", `--log-level warning --metadata "title=Two words"`));
  await settle();
  await b.evaluate(clickSelector("#quarto-section .tool-command-actions button"));
  await until("integration saved", () => b.evaluate("(window.integrationCalls || []).length === 1"), 5000);
  assert.deepEqual(JSON.parse(await b.evaluate("JSON.stringify(window.integrationCalls[0])")), {
    name: "quarto", path: "/opt/quarto/bin", args: ["--log-level", "warning", "--metadata", "title=Two words"],
  });

  // The address editor is behind an explicit details action. Diagnostics use
  // the local client and render their result as status text.
  // The address, its editor and the setup check sit on the page itself.
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Companion address"]')?.value`), "http://127.0.0.1:8763/", "the address field shows the current companion address");
  await b.evaluate(`(() => { const input = document.querySelector('[aria-label="Companion address"]'); input.value = 'http://127.0.0.1:9876/'; input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await b.evaluate(clickText("Save"));
  await until("address saved", () => b.evaluate("window.savedAddress === 'http://127.0.0.1:9876/'"), 5000);
  await until("address save feedback shown", () => b.evaluate("document.querySelector('#diagnostics-address-feedback')?.textContent.trim() === 'Saved'"), 5000);
  await b.evaluate(`window.setLocalStatus({ state: 'connected', address: 'http://127.0.0.1:9876/', capabilities: { tools: { quarto: { available: true, version: '1.6.0' } }, confinement: { kind: 'none' } } })`);
  await until("settings loaded at new address", () => b.evaluate("window.settingsCalls >= 2 && document.querySelector('#diagnostics-report button')?.disabled === false"), 5000);
  await b.evaluate(clickSelector("#diagnostics-report button"));
  await until("diagnostic report shown", () => b.evaluate("Boolean(document.querySelector('pre[role=status]'))"), 5000);
  assert.match(await b.evaluate("document.querySelector('pre[role=status]').textContent"), /quarto/);
  assert.equal(await b.evaluate("window.capabilityCalls"), 1, "diagnostics request a fresh local capability report");

  await b.evaluate(`window.setLocalStatus({ state: 'unreachable', address: 'http://127.0.0.1:9876/', capabilities: null })`);
  await until("startup and quit hidden after disconnect", () => b.evaluate("!document.querySelector('#diagnostics-startup') && ![...document.querySelectorAll('button')].some((button) => button.textContent.trim() === 'Quit companion')"), 5000);

  console.log("local-settings-browser: companion row install link, failed-popup fallback, inline address editor, inline diagnostics, and standalone-only startup/quit controls passed");
} finally {
  if (b) await b.close();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
