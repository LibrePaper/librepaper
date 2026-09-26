// The local companion settings are an important permission boundary: their
// controls describe the machine LibrePaper can use, while the Quarto switch
// asks Reader to confirm before it changes execution mode.
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
const temporary = mkdtempSync(join(tmpdir(), "librepaper-local-settings-"));
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
  import LocalAppSettings from ${JSON.stringify(join(root, "web/src/components/settings/LocalAppSettings.svelte"))};
  let localExecution = $state(false);
  const onlocalexecution = () => { window.executionRequests += 1; };
</script>
<LocalAppSettings sourceFormat="quarto" main="main.qmd" mayEdit={true}
                  {localExecution} {onlocalexecution} />
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
export function connect() { return Promise.reject(new Error("The pairing code was not accepted.")); }
export function chooseFolderBinding() { return Promise.resolve({ id: "folder", entrypoint: "main.qmd" }); }
export async function capabilities(options) {
  window.capabilityCalls = (window.capabilityCalls || 0) + 1;
  window.capabilityOptions = options;
  return { tools: { quarto: { available: true, version: "1.6.0" } }, confinement: { kind: "none" } };
}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { mount } from ${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))};
import Harness from ${JSON.stringify(harness)};
window.executionRequests = 0;
mount(Harness, { target: document.body });
`);

const mockModules = {
  name: "local-settings-test-mocks",
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
  await until("local settings mounted", () => b.evaluate("Boolean(document.querySelector('#local-status'))"), 10000);

  // An unreachable companion exposes the two install paths. Pairing details
  // and the hand-entered code stay out of the first view.
  const initial = await b.evaluate(`JSON.stringify({
    text: document.body.innerText,
    details: document.querySelectorAll('details').length,
    summaries: document.querySelectorAll('summary').length,
    pairingCode: Boolean(document.querySelector('[aria-label="Pairing code"]')),
  })`);
  const disconnected = JSON.parse(initial);
  assert.match(disconnected.text, /macOS & Linux/);
  assert.match(disconnected.text, /Windows/);
  assert.equal(disconnected.details, 0, "initial view has no collapsed details");
  assert.equal(disconnected.summaries, 0, "initial view has no disclosure summary");
  assert.equal(disconnected.pairingCode, false, "manual code entry is initially hidden");

  // A failed one-click permission window makes the manual pairing fallback
  // available, and explains why it appeared.
  await b.evaluate("window.setLocalStatus({ state: 'unauthorized', address: 'http://127.0.0.1:8763/', capabilities: null })");
  await settle();
  assert.equal(await b.evaluate("Boolean(document.querySelector('[aria-label=\"Pairing code\"]'))"), false, "manual pairing stays hidden until the one-click attempt fails");
  await b.evaluate(clickText("Connect companion"));
  await until("popup failure shown", () => b.evaluate("document.body.innerText.includes('permission window was blocked')"), 5000);
  assert.equal(await b.evaluate("Boolean(document.querySelector('[aria-label=\"Pairing code\"]'))"), true, "manual pairing appears after popup failure");

  // Once connected, the install commands give way to machine controls and a
  // Quarto permission switch. The switch asks Reader to confirm; its checked
  // state remains controlled until Reader reports the confirmed change.
  await b.evaluate(`window.setLocalStatus({ state: 'connected', address: 'http://127.0.0.1:8763/', capabilities: { tools: { quarto: { available: true, version: '1.6.0' } }, confinement: { kind: 'none' } } })`);
  await until("connected settings shown", () => b.evaluate("document.body.innerText.includes('Open companion settings')"), 5000);
  const connected = await b.evaluate(`JSON.stringify({
    text: document.body.innerText,
    switch: document.querySelector('#local-execution [role="switch"][aria-label="Allow paired Quarto documents to run local code"]')?.getAttribute('aria-checked'),
    switchLabel: document.querySelector('#local-execution').textContent,
  })`);
  const connectedView = JSON.parse(connected);
  assert.doesNotMatch(connectedView.text, /macOS & Linux|Windows/);
  assert.doesNotMatch(connectedView.text, /permission window was blocked/);
  assert.match(connectedView.text, /Open companion settings/);
  assert.match(connectedView.switchLabel, /Allow paired Quarto documents to run local code/);
  assert.equal(connectedView.switch, "false", "local execution starts off");
  await b.evaluate(clickSelector('#local-execution [role="switch"]'));
  await settle();
  assert.equal(await b.evaluate("window.executionRequests"), 1, "the permission choice calls Reader's confirmation handler");
  assert.equal(await b.evaluate(`document.querySelector('#local-execution [role="switch"]')?.getAttribute('aria-checked')`), "false", "controlled switch stays off until confirmation updates Reader");

  // The address editor is behind an explicit details action. Diagnostics use
  // the local client and render their result as status text.
  await b.evaluate(clickText("Connection details"));
  await until("custom address action shown", () => b.evaluate("document.body.innerText.includes('Custom companion address')"), 5000);
  await b.evaluate(clickText("Custom companion address"));
  await until("address dialog shown", () => b.evaluate("document.body.innerText.includes('Save address')"), 5000);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Custom companion address"]')?.value`), "http://127.0.0.1:8763/", "address dialog displays the current companion address");
  await b.evaluate(`(() => { const input = document.querySelector('[aria-label="Custom companion address"]'); if (!input) throw new Error('Missing custom address input'); input.value = 'http://127.0.0.1:9876/'; input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await b.evaluate(clickText("Save address"));
  await until("address saved", () => b.evaluate("window.savedAddress === 'http://127.0.0.1:9876/'"), 5000);
  await b.evaluate(clickText("Check local setup"));
  await until("diagnostic report shown", () => b.evaluate("Boolean(document.querySelector('pre[role=status]'))"), 5000);
  assert.match(await b.evaluate("document.querySelector('pre[role=status]').textContent"), /quarto/);
  assert.equal(await b.evaluate("window.capabilityCalls"), 1, "diagnostics request a fresh local capability report");

  console.log("local-settings-browser: install commands, failed-popup fallback, connected permission confirmation, address dialog, and diagnostics passed");
} finally {
  if (b) await b.close();
  server.close();
  rmSync(temporary, { recursive: true, force: true });
}
