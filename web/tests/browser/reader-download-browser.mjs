// File > Download in the real Reader, for a Markdown project: the submenu lists
// the four downloads, HTML, DOCX and PDF each save a file made in the browser
// through Pandoc and Typst, and View > PDF previews without a "PDF needs" card.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, pause, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const names = ["markdown", "citations", "typst", "pandoc"];
const files = Object.fromEntries(names.map((name) => [name, join(root, "web/wasm", name + ".wasm")]));
const missing = names.filter((name) => !existsSync(files[name]));
if (missing.includes("pandoc") || missing.includes("typst")) {
  console.log(`reader-download-browser: no ${missing.join(", ")} module in web/wasm; skipping (fetch the wasm modules first)`);
  process.exit(0);
}

// The renderer checks a module against the digest in its URL, as the mirror lays it out.
const urls = {}, served = new Map();
for (const name of names) {
  if (!existsSync(files[name])) continue;
  const bytes = readFileSync(files[name]);
  urls[name] = `/wasm/${createHash("sha256").update(bytes).digest("hex")}/${name}.wasm`;
  served.set(urls[name], bytes);
}

const SLUG = "paper";
const source = "# Download check\n\nA paragraph with inline math $x^2$ in it.\n";
const temp = mkdtempSync(join(tmpdir(), "librepaper-reader-download-"));
const entry = join(temp, "entry.js"), room = join(temp, "room.js"), out = join(temp, "build");

writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const text = new LoroText(); text.insert(0, ${JSON.stringify(source)});
server.getMap("files").setContainer("main", text);
server.getMap("paths").set("main", "main.md");
server.getMap("meta").set("main", "main");
server.commit();
const update = encode(server.export({ mode: "update" }));
const vector = encode(server.oplogVersion().encode());
export function openRoom(_slug, {onMessage, onConnected}) {
  queueMicrotask(() => onConnected(true));
  return {
    send(message) { if (message.type === "doc-open") queueMicrotask(() => onMessage({type:"doc-state", protocol:"librepaper.room.v3", vector, durableVector: vector, updates:[update]})); return {ok:true}; },
    sendLive() { return {ok:true}; },
    close() {}
  };
}`);
writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
globalThis.LIBREPAPER_MODULES = ${JSON.stringify(urls)};
window.testErrors = [];
addEventListener('error', event => window.testErrors.push(event.error?.stack || event.message));
addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
// Every download goes through an anchor with a download attribute (saveBlob).
// Its bytes are read at once, before the object URL can be revoked.
window.downloads = [];
const realClick = HTMLAnchorElement.prototype.click;
HTMLAnchorElement.prototype.click = function () {
  if (this.hasAttribute('download')) {
    const name = this.getAttribute('download'), href = this.href;
    window.downloads.push(fetch(href).then(r => r.arrayBuffer()).then(buffer => {
      const bytes = new Uint8Array(buffer);
      return { name, size: bytes.length, head: String.fromCharCode(...bytes.subarray(0, 4)), text: new TextDecoder().decode(bytes.subarray(0, 400000)) };
    }));
    return;
  }
  return realClick.call(this);
};
const {default: Reader}=await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
const {mount}=await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
mount(Reader,{target:document.body});
`);

const json = (response, value, status = 200) => {
  response.statusCode = status; response.setHeader("content-type", "application/json"); response.end(JSON.stringify(value));
};
let http, tab;
try {
  await build({ configFile: false, define: KATEX_DEFINE, root: join(root, "web"), resolve: { alias: loroAlias }, plugins: [tailwindcss(), svelte(), {
    name: "download-test-room", enforce: "pre", resolveId(id) {
      if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
    },
  }], build: { outDir: out, emptyOutDir: true, minify: false, lib: { entry, formats: ["es"], fileName: () => "check.js", cssFileName: "check" } }, logLevel: "error" });
  http = createServer((request, response) => {
    if (serveKatex(request, response)) return;
    if (served.has(request.url)) { response.setHeader("content-type", "application/wasm"); return response.end(served.get(request.url)); }
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/api/me") return json(response, { name: "Editor", providers: [] });
    if (url.pathname === `/api/documents/${SLUG}`) return json(response, {
      title: "Download test", created_at: "download-test", role: "editor", source_format: "markdown",
      docs_origin: `http://${request.headers.host}`,
    });
    if (url.pathname.startsWith("/api/")) return json(response, {});
    if (url.pathname.startsWith("/raw/") || url.pathname.startsWith("/pdf/")) { response.setHeader("content-type", "text/html"); return response.end("<!doctype html><body></body>"); }
    if (url.pathname === `/docs/${SLUG}`) {
      response.setHeader("content-type", "text/html");
      return response.end('<!doctype html><html data-theme="librepaper"><head><link rel="stylesheet" href="/check.css"></head><body><script type="module" src="/check.js"></script></body></html>');
    }
    try {
      const file = join(out, url.pathname.slice(1));
      response.setHeader("content-type", contentType(file)); response.end(readFileSync(file));
    } catch { response.statusCode = 404; response.end(); }
  });
  await new Promise((resolve, reject) => { http.once("error", reject); http.listen(0, "127.0.0.1", resolve); });
  const origin = `http://127.0.0.1:${http.address().port}`;
  tab = await browser("chromium", join(temp, "profile"), 36000 + Math.floor(Math.random() * 1000));
  await tab.resize(1400, 900);
  await tab.navigate(`${origin}/docs/${SLUG}`);
  await until("the bar and the source", () => tab.evaluate(`Boolean(document.querySelector('[data-menubar="file"]')) && Boolean(document.querySelector('.cm-content')?.textContent.includes('Download check'))`), 15000);

  const flush = () => tab.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  // Zag reads real pointer events; a synthetic click() does not open a menu.
  const mouse = async (x, y, types) => {
    for (const type of types) await tab.command("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, buttons: type === "mousePressed" ? 1 : 0 });
    await flush();
  };
  // The rect of the first visible element matching a script that returns it, or null.
  const where = (finder) => tab.evaluate(`(() => {
    const el = (${finder})();
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2 || r.right <= 0 || r.bottom <= 0 || r.left >= innerWidth || r.top >= innerHeight) return null;
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  // Menus stay mounted while closed, so wait for an on-screen box, not for existence.
  const visible = (label, finder) => until(label, () => where(finder), 8000);
  const item = (text) => `() => [...document.querySelectorAll('[role="menuitem"]')].find(n => n.textContent.trim().startsWith(${JSON.stringify(text)}) && n.getBoundingClientRect().width > 1)`;
  const trigger = (id) => `() => document.querySelector('[data-menubar="${id}"]')`;

  const openMenu = async (id, finder) => {
    const spot = await until(`the ${id} trigger`, () => where(trigger(id)), 8000);
    await mouse(spot.x, spot.y, ["mouseMoved", "mousePressed", "mouseReleased"]);
    const target = await visible(`the ${id} menu`, finder);
    await pause(500);
    return target;
  };
  const openDownload = async () => {
    await openMenu("file", item("Download"));
    const sub = await where(item("Download"));
    await mouse(sub.x, sub.y, ["mouseMoved"]);
    await visible("the Download submenu", item("HTML"));
    await pause(500);
  };
  const choose = async (text) => {
    const spot = await where(item(text));
    await mouse(spot.x, spot.y, ["mouseMoved", "mousePressed", "mouseReleased"]);
  };
  const closeMenus = async () => {
    await tab.command("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await tab.command("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await flush();
  };

  // 1. The submenu: its entries, in order, none disabled.
  await openDownload();
  const entries = await tab.evaluate(`(() => {
    const labels = ["Source (.zip)", "PDF", "HTML", "DOCX"];
    const found = [...document.querySelectorAll('[role="menuitem"]')]
      .filter(n => n.getBoundingClientRect().width > 1 && labels.includes(n.querySelector('.menuitem-label')?.textContent.trim()));
    return found.map(n => ({
      label: n.querySelector('.menuitem-label').textContent.trim(), top: n.getBoundingClientRect().top,
      disabled: n.hasAttribute('data-disabled') || n.getAttribute('aria-disabled') === 'true',
    }));
  })()`);
  entries.sort((a, b) => a.top - b.top);
  assert.deepEqual(entries.map((e) => e.label), ["Source (.zip)", "PDF", "HTML", "DOCX"], "the Download submenu lists the four downloads in order");
  assert.deepEqual(entries.filter((e) => e.disabled), [], "no download is disabled");
  console.log("reader-download: the Download submenu lists Source, PDF, HTML and DOCX, all enabled");

  const taken = async (count) => {
    await until(`download ${count}`, () => tab.evaluate(`window.downloads.length >= ${count}`), 120000);
    return tab.evaluate(`window.downloads[${count - 1}]`);
  };

  // 3. HTML.
  await choose("HTML");
  const html = await taken(1);
  assert.equal(html.name, `${SLUG}.html`);
  assert.ok(html.text.includes("Download check"), "the HTML holds the heading");
  console.log("reader-download: HTML saved");

  // 4. DOCX, the first Pandoc load.
  await closeMenus(); await pause(300);
  await openDownload();
  await choose("DOCX");
  const docx = await taken(2);
  assert.equal(docx.name, `${SLUG}.docx`);
  assert.equal(docx.head.slice(0, 2), "PK", "the DOCX is a zip");
  console.log("reader-download: DOCX saved");

  // 5. PDF.
  await closeMenus(); await pause(300);
  await openDownload();
  await choose("PDF");
  const pdf = await taken(3);
  assert.equal(pdf.name, `${SLUG}.pdf`);
  assert.equal(pdf.head, "%PDF", "the PDF starts with %PDF");
  console.log("reader-download: PDF saved");

  // 6. View > PDF previews in the browser. The harness serves no real PDF
  // viewer, so only the frame path and the absence of an error are checked.
  await closeMenus(); await pause(300);
  await openMenu("view", item("PDF"));
  await choose("PDF");
  await until("the preview frame is the PDF viewer", () => tab.evaluate(`/\\/pdf\\b/.test(document.querySelector('iframe[title="Document"]')?.src || '')`), 60000);
  await pause(1500);
  const body = await tab.evaluate("document.body.innerText");
  assert.ok(!/PDF needs/i.test(body), "no \"PDF needs\" card: " + body.slice(0, 300));
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
  console.log("reader-download: View > PDF shows the PDF frame without a \"PDF needs\" card");
} catch (error) {
  if (tab) console.error(await tab.evaluate(`JSON.stringify({errors: window.testErrors, text: document.body.innerText.slice(0, 600)})`).catch(() => ""));
  throw error;
} finally {
  await tab?.close(); http?.close(); removeTemporary(temp);
}
