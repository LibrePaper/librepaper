// The desktop app settings are an important permission boundary: their
// controls describe the machine LibrePaper can use, and a custom Quarto
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
  import DesktopSettings from ${JSON.stringify(join(root, "web/src/components/settings/DesktopSettings.svelte"))};
  import ToolCommand from ${JSON.stringify(join(root, "web/src/components/settings/ToolCommand.svelte"))};
  import { createMachineView } from ${JSON.stringify(join(root, "web/src/lib/desktop/machine.svelte.js"))};
  const view = createMachineView();
  $effect(() => view.start());
</script>
<DesktopSettings {view} />
<section id="quarto-section"><ToolCommand name="quarto" /></section>
`);

writeFileSync(clientMock, `
export const INSTALL_URL = "https://librepaper.org/desktop-app.html";
export const DESKTOP_ONLY_MESSAGE = "This needs the LibrePaper desktop app.";
export const desktop = () => window.__LIBREPAPER_DESKTOP__ ?? null;
export function chooseFolderBinding() { return Promise.resolve({ id: "folder", entrypoint: "main.qmd" }); }
export async function manage(path, { method = "GET", body } = {}) {
  window.manageCalls = [...(window.manageCalls || []), { path, method, body }];
  if (path === "/state") return { version: "1.0.0", jobs: [], previews: [], sessions: [], bindings: [], settings: { integrations: { quarto: { path: null, args: [] }, calepin: { path: null, args: [] } } } };
  if (path === "/settings" && method === "PUT") return null;
  throw new Error("unexpected management call " + path);
}
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
    fields: Boolean(document.querySelector('#quarto-executable input')?.disabled && document.querySelector('#quarto-arguments input')?.disabled),
  })`);
  const browserView = JSON.parse(initial);
  assert.equal(browserView.install, "https://librepaper.org/desktop-app.html", "the notice links to the one install page");
  assert.match(browserView.open, /^librepaper:\/\/open\?url=http/, "the page offers to open itself in the desktop app");
  assert.match(browserView.text, /This needs the LibrePaper desktop app\./);
  assert.doesNotMatch(browserView.text, /curl|librepaper-installer|[Cc]ompanion/, "the page carries no install command and no old name");
  assert.equal(browserView.details, 0, "initial view has no collapsed details");
  assert.equal(browserView.fields, true, "Quarto fields stay unavailable in a browser");
  assert.equal(await b.evaluate("Boolean(document.querySelector('#desktop-activity'))"), false, "a browser shows no running work");

  // Inside the desktop app the machine controls show and the Quarto section
  // offers its own command.
  await b.navigate(`http://127.0.0.1:${port}/`);
  await until("desktop settings shown", () => b.evaluate("Boolean(document.querySelector('#desktop-activity')) && document.querySelector('#quarto-executable input')?.disabled === false"), 10000);
  const connectedText = await b.evaluate("document.body.innerText");
  assert.equal(await b.evaluate("document.querySelector('.desktop-only')"), null, "the desktop app needs no install link");
  assert.doesNotMatch(connectedText, /Build presets|Available tools|Quit|Start at login|Waiting for your answer|[Cc]ompanion/);
  assert.match(connectedText, /Nothing running\./);

  // A custom command is split into arguments, quotes kept together.
  const type = (label, value) => `(() => { const input = document.querySelector('#quarto-section [aria-label="${label}"]'); input.value = ${JSON.stringify(value)}; input.dispatchEvent(new Event('input', { bubbles: true })); })()`;
  await b.evaluate(type("Executable path", "/opt/quarto/bin"));
  await b.evaluate(type("Arguments", `--log-level warning --metadata "title=Two words"`));
  await settle();
  await b.evaluate(clickSelector("#quarto-section .tool-command-actions button"));
  await until("integration saved", () => b.evaluate("(window.manageCalls || []).some((call) => call.method === 'PUT')"), 5000);
  assert.deepEqual(JSON.parse(await b.evaluate("JSON.stringify(window.manageCalls.find((call) => call.method === 'PUT').body)")), {
    integrations: { quarto: { path: "/opt/quarto/bin", args: ["--log-level", "warning", "--metadata", "title=Two words"] } },
  });

  console.log("local-settings-browser: browser notice, desktop app controls and integration commands passed");
} finally {
  if (b) await b.close();
  server.close();
  removeTemporary(temporary);
}
