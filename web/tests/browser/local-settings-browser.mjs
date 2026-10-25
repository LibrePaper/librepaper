// The desktop app settings are an important permission boundary: their
// controls describe the machine LibrePaper can use, and a custom render
// command is sent to the desktop app, which confirms it on this computer.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = dirname(dirname(dirname(here)));
const temporary = mkdtempSync(join(tmpdir(), "librepaper-local-settings-"));
const output = join(temporary, "build");
const entry = join(temporary, "entry.js");
const harness = join(temporary, "Harness.svelte");
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
  import ToolsSettings from ${JSON.stringify(join(root, "web/src/components/settings/ToolsSettings.svelte"))};
  import ToolCommand from ${JSON.stringify(join(root, "web/src/components/settings/ToolCommand.svelte"))};
  import { createMachineView } from ${JSON.stringify(join(root, "web/src/lib/desktop/machine.svelte.js"))};
  const view = createMachineView();
  $effect(() => view.start());
</script>
<ToolsSettings {view} />
<section id="quarto-section"><ToolCommand format="quarto" output="html" /></section>
`);

writeFileSync(clientMock, `
export const INSTALL_URL = "https://librepaper.org/desktop-app.html";
export const DESKTOP_ONLY_MESSAGE = "This needs the LibrePaper desktop app.";
export const desktop = () => window.__LIBREPAPER_DESKTOP__ ?? null;
export function chooseFolderBinding() { return Promise.resolve({ id: "folder", entrypoint: "main.qmd" }); }
export async function manage(path, { method = "GET", body } = {}) {
  window.manageCalls = [...(window.manageCalls || []), { path, method, body }];
  if (path === "/state") return { version: "1.0.0", jobs: [], sessions: [], bindings: [], settings: { renderers: { "quarto.html": { command: ["quarto", "render", "{input}", "--to", "html"], result: "{stem}.html", edited: false, found: true, version: "1.6.0" } } } };
  throw new Error("unexpected management call " + path);
}
export async function capabilities() { return { tools: {}, builders: [] }; }
export function forgetCapabilities() {}
export async function shellAction() {}
export async function desktopReport() { return ""; }
export const rendererKey = (format, output) => format + "." + output;
export async function setRenderer(key, { command, result }) {
  window.rendererCalls = [...(window.rendererCalls || []), { key, command, result }];
  return { command, result };
}
export async function resetRenderer() { return {}; }
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
    if (source.endsWith("/lib/desktop/client.js") || (source === "./client.js" && importer?.endsWith("/lib/desktop/machine.svelte.js"))) return clientMock;
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
// Inside the desktop app the page global exists; the plain page is a browser.
const desktopPage = page.replace("<body>", "<body><script>window.__LIBREPAPER_DESKTOP__ = { base: 'x', version: 'test' }</script>");
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
  response.end(request.url.startsWith("/plain") ? page : desktopPage);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
const debugPort = await freePort();
let b;
const settle = () => new Promise((resolve) => setTimeout(resolve, 80));
const clickSelector = (selector) => `(() => { const node = document.querySelector(${JSON.stringify(selector)}); if (!node) throw new Error('Could not find control: ' + ${JSON.stringify(selector)}); node.click(); return true; })()`;

try {
  b = await browser("chromium", join(temporary, "chrome"), debugPort);
  // A browser outside the desktop app gets one notice with an install link and
  // a link that opens the page in the desktop app.
  await b.navigate(`http://127.0.0.1:${port}/plain`);
  await until("browser notice shown", () => b.evaluate("Boolean(document.querySelector('.desktop-only'))"), 10000);
  const initial = await b.evaluate(`JSON.stringify({
    text: document.body.innerText,
    details: document.querySelectorAll('details').length,
    install: document.querySelector('.desktop-only a[href*="desktop-app"]')?.href,
    open: document.querySelector('.desktop-only a[href^="librepaper://open?url="]')?.getAttribute('href'),
    fields: Array.from(document.querySelectorAll('#render-quarto-html input')).every((input) => input.disabled),
  })`);
  const browserView = JSON.parse(initial);
  assert.equal(browserView.install, "https://librepaper.org/desktop-app.html", "the notice links to the one install page");
  assert.match(browserView.open, /^librepaper:\/\/open\?url=http/, "the page offers to open itself in the desktop app");
  assert.match(browserView.text, /This needs the LibrePaper desktop app\./);
  assert.doesNotMatch(browserView.text, /curl|librepaper-installer|[Cc]ompanion/, "the page carries no install command and no old name");
  assert.equal(browserView.details, 0, "initial view has no collapsed details");
  assert.equal(browserView.fields, true, "the Quarto command stays unavailable in a browser");
  assert.equal(await b.evaluate("Boolean(document.querySelector('#tools-activity'))"), false, "a browser shows no running work");

  // Inside the desktop app the machine controls show and the Quarto section
  // offers its own command.
  await b.navigate(`http://127.0.0.1:${port}/`);
  await until("desktop settings shown", () => b.evaluate("Boolean(document.querySelector('#tools-programs')) && document.querySelector('#render-quarto-html input')?.disabled === false"), 10000);
  const connectedText = await b.evaluate("document.body.innerText");
  assert.equal(await b.evaluate("document.querySelector('.desktop-only')"), null, "the desktop app needs no install link");
  assert.doesNotMatch(connectedText, /Build presets|Available tools|Quit|Start at login|Waiting for your answer|[Cc]ompanion/);
  assert.equal(await b.evaluate("Boolean(document.querySelector('#tools-activity'))"), false, "an idle desktop app shows no Activity section");

  // The command shows whole on one line, with whether the program was found.
  const shown = JSON.parse(await b.evaluate(`JSON.stringify({
    command: document.querySelector('#render-quarto-html [aria-label="HTML command for quarto"]').value,
    result: document.querySelector('#render-quarto-html [aria-label="Result file for quarto html"]').value,
    status: document.querySelector('#render-quarto-html [role=status]').textContent.trim(),
  })`));
  assert.equal(shown.command, "quarto render {input} --to html");
  assert.equal(shown.result, "{stem}.html");
  assert.match(shown.status, /Found · 1\.6\.0/);

  // A custom command is split into arguments, quotes kept together.
  const type = (label, value) => `(() => { const input = document.querySelector('#quarto-section [aria-label="${label}"]'); input.value = ${JSON.stringify(value)}; input.dispatchEvent(new Event('input', { bubbles: true })); })()`;
  await b.evaluate(type("HTML command for quarto", `/opt/quarto/bin/quarto render {input} --metadata "title=Two words" --to html`));
  await b.evaluate(type("Result file for quarto html", "out/{stem}.html"));
  await settle();
  await b.evaluate(clickSelector("#quarto-section .tool-command-actions button"));
  await until("renderer saved", () => b.evaluate("(window.rendererCalls || []).length > 0"), 5000);
  assert.deepEqual(JSON.parse(await b.evaluate("JSON.stringify(window.rendererCalls[0])")), {
    key: "quarto.html", command: ["/opt/quarto/bin/quarto", "render", "{input}", "--metadata", "title=Two words", "--to", "html"], result: "out/{stem}.html",
  });

  console.log("local-settings-browser: browser notice, desktop app controls and render commands passed");
} finally {
  if (b) await b.close();
  server.close();
  removeTemporary(temporary);
}
