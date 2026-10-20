// Where a setting lives, in the dialog itself. A tool that builds the
// document is configured on the Rendering page; the Tools page holds the
// programs on this computer, the project folders the app is bound to and, only
// while something runs, its activity; and the Account page carries the server.
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
  window.show = (page) => { category = page; open = true; };
  window.applied = [];
</script>
<SettingsDialog bind:open bind:category userId="user"
                onbuildpreferences={(format, next) => { window.applied.push(next); }}
                onquartooptions={(next) => { window.applied.push(next); }}
                remoteConnected={true} />
`);

writeFileSync(clientMock, `
export const INSTALL_URL = "https://librepaper.org/desktop-app.html";
export const DESKTOP_ONLY_MESSAGE = "This needs the LibrePaper desktop app.";
export const desktop = () => window.__LIBREPAPER_DESKTOP__ ?? null;
export function chooseFolderBinding() { return Promise.resolve({ id: "folder", entrypoint: "main.qmd" }); }
export async function capabilities() { return {}; }
export function forgetCapabilities() {}
export async function shellAction() {}
export async function desktopReport() { return "Desktop app test report"; }
export async function manage(path) {
  if (path !== "/state") throw new Error("unexpected management call " + path);
  return {
    version: "1.0.0", jobs: [], previews: [], sessions: [], bindings: [],
    settings: { integrations: { quarto: { path: null, args: [] }, calepin: { path: null, args: [] } } },
    tools: {
      tools: { quarto: { available: true, version: "1.6.0" } },
      calepin: { available: true, version: "0.3.0" },
      zotero: { available: true, version: "7.0" },
      builders: [{ id: "quarto", available: true, version: "1.6.0", outputs: ["html", "pdf"], operations: [{ kind: "build", workspace_modes: ["snapshot"] }] }],
    },
  };
}
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
    if (source.endsWith("/lib/desktop/client.js") || (source === "./client.js" && importer?.endsWith("/lib/desktop/machine.svelte.js"))) return clientMock;
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

try {
  b = await browser("chromium", join(temporary, "chrome"), debugPort);
  await b.navigate(`http://127.0.0.1:${port}/`);
  const heading = () => b.evaluate(`document.querySelector(".settings-category")?.textContent.trim() || ""`);
  const present = (ids) => b.evaluate(`JSON.stringify(${JSON.stringify(ids)}.filter((id) => document.querySelector(".settings-body #" + id)))`).then(JSON.parse);
  const show = async (pageId, format, says) => {
    await b.evaluate(`window.show(${JSON.stringify(pageId)})`);
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
  const everything = ["render-latex-preview", "render-latex-engine", "render-latex-files", "render-typst-preview", "render-markdown-preview", "render-markdown-tool", "render-quarto-preview", "rendering-profile", "rendering-parameters",
    "tools-programs", "tools-quarto", "tools-calepin", "tools-zotero", "quarto-executable", "quarto-arguments",
    "tools-folders", "tools-activity", "diagnostics-report",
    "remote-status", "remote-address", "storage-account", "account-erase"];
  const renderingRows = ["render-latex-preview", "render-latex-engine", "render-latex-files", "render-typst-preview", "render-markdown-preview", "render-markdown-tool", "render-quarto-preview", "rendering-profile", "rendering-parameters"];
  const renderingSections = ["LaTeX", "Typst", "Markdown", "Quarto"];

  // The navigation, in order, for a visitor on a Quarto document.
  await until("harness mounted", () => b.evaluate("typeof window.show === \"function\""), 10000);
  await show("rendering", "quarto", "Rendering");
  assert.deepEqual(JSON.parse(await b.evaluate(`JSON.stringify([...document.querySelectorAll(".settings-nav-item")].map((node) => node.textContent.trim()))`)),
    ["Appearance", "Editor", "Rendering", "Tools", "AI agents", "Backups", "Account", "Diagnostics"]);

  // Settings is a page, not a card: it covers the window, Back closes it and
  // so does Escape.
  assert.equal(await b.evaluate(`(() => { const r = document.querySelector(".modal-page").getBoundingClientRect(); return r.top === 0 && r.left === 0 && r.width === innerWidth && r.height === innerHeight; })()`), true, "settings covers the window");
  assert.equal(await b.evaluate(`(() => { const r = document.querySelector(".modal-page-body").getBoundingClientRect(); return r.left === 0 && r.width === innerWidth; })()`), true, "the settings body uses the full width");
  assert.equal(await b.evaluate(`document.querySelector(".modal-back")?.textContent.trim()`), "Back");
  await b.evaluate(`document.querySelector(".modal-back").click()`);
  await until("Back closes settings", () => b.evaluate(`!document.querySelector('[role="dialog"][data-state="open"]')`), 5000);
  await show("rendering", "quarto", "Rendering");
  // The dialog listens for Escape a moment after it opens, so press until it closes.
  await until("Escape closes settings", async () => {
    await b.command("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await b.command("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    return b.evaluate(`!document.querySelector('[role="dialog"][data-state="open"]')`);
  }, 5000);
  await show("rendering", "quarto", "Rendering");

  // Rendering shows one section per format, with the global build options visible in each.
  await until("the Quarto profile rows", () => present(["rendering-profile"]).then((found) => found.length === 1), 5000);
  assert.deepEqual(await present(everything), renderingRows);
  assert.deepEqual(await subheads(), renderingSections);
  assert.equal(await b.evaluate(`document.querySelector(".settings-group-title") === null`), true);
  // The programs the desktop app finds live on the Tools page, not here.
  assert.equal(await b.evaluate(`[...document.querySelectorAll(".settings-body .setting-status-pill")].some((pill) => /Running|Not running|Needs approval|Blocked|Update needed/.test(pill.textContent))`), false, "Rendering has no program status pill");
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

  // The Tools page is the programs on this computer. Zotero feeds a document
  // without building it, and has no command of its own. Quarto lists its
  // executable and arguments behind Details. The version sits beside the title.
  await show("tools", "quarto", "Tools");
  await until("the program rows", () => present(["tools-zotero"]).then((found) => found.length === 1), 5000);
  await capture("tools");
  assert.deepEqual(await present(everything), ["tools-programs", "tools-quarto", "tools-calepin", "tools-zotero"], "no bindings, so no folders; nothing runs, so no Activity section");
  assert.deepEqual(await subheads(), ["Programs"], "no bindings and nothing running, so only Programs");
  assert.equal(await b.evaluate(`document.querySelector(".settings-scope")?.textContent.trim()`), "Version 1.0.0 · This computer");
  assert.match(await b.evaluate(`document.querySelector("#tools-zotero .setting-status-pill")?.textContent`), /Available · 7\.0/);
  assert.equal(await b.evaluate(`document.querySelector("#tools-zotero .tool-row-toggle")`), null, "Zotero has no command to edit");
  assert.equal(await b.evaluate(`document.querySelector("#quarto-executable") === null`), true, "the executable row starts hidden");
  await b.evaluate(`document.querySelector("#tools-quarto .tool-row-toggle").click()`);
  await until("the Quarto command rows", () => present(["quarto-executable", "quarto-arguments"]).then((found) => found.length === 2), 5000);
  assert.deepEqual(await present(everything), ["tools-programs", "tools-quarto", "tools-calepin", "tools-zotero", "quarto-executable", "quarto-arguments"]);

  // The desktop app offers its Diagnostics page with Open logs among the buttons.
  await show("diagnostics", "quarto", "Diagnostics");
  assert.equal(await b.evaluate(`document.querySelector("#diagnostics-report .diagnostics-buttons")?.textContent.includes("Open logs")`), true, "the desktop app offers Open logs");

  // A visitor's Account page is the server; storage and erasure need an account.
  await show("account", "quarto", "Account");
  await capture("account");
  assert.deepEqual(await present(everything), ["remote-status", "remote-address"]);
  assert.match(await b.evaluate(`document.querySelector("#remote-status .setting-status-pill")?.textContent`), /Connected/);
  assert.match(await b.evaluate(`document.querySelector("#remote-status").textContent`), /Remote connection/);

  // In a browser the desktop app pages give one notice and the rendering
  // choices stay editable.
  await b.navigate(`http://127.0.0.1:${port}/plain`);
  await until("harness mounted in a browser", () => b.evaluate("typeof window.show === \"function\""), 10000);
  await show("rendering", "quarto", "Rendering");
  await capture("rendering-browser");
  assert.deepEqual(await subheads(), renderingSections);
  await until("browser rendering settings", () => present(["render-latex-engine"]).then((found) => found.length === 1), 5000);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="LaTeX engine"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto profile"]').disabled`), false);
  assert.equal(await b.evaluate(`document.querySelector('[aria-label="Quarto parameters"]').disabled`), false);
  for (const [pageId, says] of [["tools", "Tools"], ["agents", "AI agents"], ["backups", "Backups"]]) {
    await show(pageId, "quarto", says);
    await until(`the browser notice on ${says}`, () => b.evaluate(`Boolean(document.querySelector(".settings-body .desktop-only a"))`), 5000);
    assert.equal(await b.evaluate(`document.querySelector(".settings-body .desktop-only-message").textContent.trim()`), "This needs the LibrePaper desktop app.");
    assert.equal(await b.evaluate(`document.querySelector(".settings-body .desktop-only a")?.getAttribute("href")`), "https://librepaper.org/desktop-app.html", `${says} links the install page`);
    assert.match(await b.evaluate(`document.querySelector(".settings-body .desktop-only a[href^='librepaper://open?url=']")?.textContent || ""`), /Open in the desktop app/);
    assert.equal(await b.evaluate(`document.querySelector("#quarto-executable") === null`), true, `${says} shows no command fields in a browser`);
  }
  await capture("tools-browser");

  await show("diagnostics", "quarto", "Diagnostics");
  assert.equal(await b.evaluate(`[...document.querySelectorAll(".settings-nav-item")].some((node) => node.textContent.trim() === "Diagnostics")`), true, "a browser is offered Diagnostics");
  assert.equal(await b.evaluate(`[...document.querySelectorAll("#diagnostics-report button")].some((node) => node.textContent.trim() === "Copy diagnostics")`), true, "the panel has Copy diagnostics");
  await b.evaluate(`document.querySelector("#diagnostics-report details summary").click()`);
  await until("the report fills", () => b.evaluate(`/## App/.test(document.querySelector("#diagnostics-report textarea").value) && /## Environment/.test(document.querySelector("#diagnostics-report textarea").value)`), 5000);

  console.log("settings-pages-browser: Rendering shows one section per format with a default preview, Tools lists the programs with Quarto commands behind Details, a browser gets one notice on each desktop page, and a browser has a Diagnostics page");
} finally {
  if (b) await b.close();
  server.close();
  removeTemporary(temporary);
}
