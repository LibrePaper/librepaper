// Browser acceptance for the document outline.
//
// The Reader is real. Only its room and HTTP shell are local test doubles: the
// room is backed by Yjs, so file changes take the same path as a collaborator's
// update. Keeping the main file HTML lets this check exercise Markdown,
// Quarto, Typst and LaTeX source files without requiring five compilers.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until } from "../tools/browser-driver.mjs";

const root = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-outline-browser-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");
const yjs = join(root, "web/node_modules/yjs/dist/yjs.mjs");

// A deliberately long preamble makes a jump observable in both the editor's
// caret line and its own scroll container. Each syntax has a top-level heading,
// a child heading, and a later top-level heading to exercise active-heading
// resets as well as parsing.
writeFileSync(room, `
import * as Y from ${JSON.stringify(yjs)};
const encode = bytes => btoa(String.fromCharCode(...bytes));
const decode = text => Uint8Array.from(atob(text), c => c.charCodeAt(0));
const rooms = new Map();

const long = (lines, body) => [...Array.from({length: 55}, (_, i) => "preamble line " + i), ...lines, body].join("\\n");
const initial = {
  "main.html": ["<!doctype html>", "<html><body>", ...Array.from({length: 55}, (_, i) => "<!-- preamble " + i + " -->"), "<h1>HTML heading</h1>", "<h2>HTML child</h2>", "<h1>HTML next</h1>", "<p>Rendered main document.</p>", "</body></html>"].join("\\n"),
  "notes.md": long(["# Markdown heading", "", "## Markdown subsection", "", "# Markdown next"], "Markdown source."),
  "chapter.qmd": long(["# Quarto heading", "", "## Quarto subsection", "", "# Quarto next"], "Quarto source."),
  "paper.typ": long(["= Typst heading", "", "== Typst subsection", "", "= Typst next"], "Typst source."),
  "paper.tex": long(["\\\\section{LaTeX heading}", "", "\\\\subsection{LaTeX subsection}", "", "\\\\section{LaTeX next}"], "LaTeX source."),
  "refs.bib": "@article{one, title={No outline here}}",
};

function makeRoom(slug) {
  const doc = new Y.Doc();
  const files = doc.getMap("files");
  const paths = doc.getMap("paths");
  const meta = doc.getMap("meta");
  for (const [path, value] of Object.entries(initial)) {
    const id = path === "main.html" ? "main" : path;
    const text = new Y.Text(value);
    files.set(id, text);
    paths.set(id, path);
  }
  meta.set("main", "main");
  return { doc, files, paths, meta };
}

function bytes(update) { return new Uint8Array(update); }
function installRoom(slug, callbacks) {
  let current = rooms.get(slug);
  if (!current) {
    current = makeRoom(slug);
    rooms.set(slug, current);
  }
  current.callback = callbacks.onMessage;
  return current;
}

export function openRoom(slug, {onMessage, onConnected}) {
  const room = installRoom(slug, {onMessage});
  window.roomSent = [];
  window.roomControl = {
    remoteAppend(path, value) {
      const id = [...room.paths.entries()].find(([, name]) => name === path)?.[0];
      const text = id && room.files.get(id);
      if (!(text instanceof Y.Text)) throw new Error("missing room file " + path);
      room.doc.transact(() => text.insert(text.length, "\\n\\n" + value), "remote-peer");
    },
    snapshot(path) {
      const id = [...room.paths.entries()].find(([, name]) => name === path)?.[0];
      return id ? room.files.get(id)?.toString() || "" : "";
    },
  };
  const listener = (update, origin) => {
    if (origin === "remote-peer") room.callback?.({type: "y-update", update: encode(update)});
  };
  room.doc.on("update", listener);
  queueMicrotask(() => onConnected(true));
  return {
    send(message) {
      window.roomSent.push(message);
      if (message.type === "y-open") {
        queueMicrotask(() => onMessage({type: "y-state", update: encode(Y.encodeStateAsUpdate(room.doc)), count: 2}));
      } else if (message.type === "y-update") {
        Y.applyUpdate(room.doc, bytes(decode(message.update)), "browser");
        queueMicrotask(() => onMessage({type: "y-ack", seq: message.seq}));
      }
      return {ok: true};
    },
    sendLive(message) { window.roomSent.push(message); return {ok: true}; },
    close() { room.doc.off("update", listener); },
  };
}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root, "web/src/styles/app.css"))};
import { EditorView } from ${JSON.stringify(join(root, "web/node_modules/@codemirror/view/dist/index.js"))};
window.editorState = () => {
  const view = EditorView.findFromDOM(document.querySelector('.cm-editor'));
  return { text: view.state.doc.toString(), caret: view.state.selection.main.head };
};
window.testErrors = [];
window.resizeNotifications = 0;
window.addEventListener('error', event => {
  // Pane changes can defer a ResizeObserver delivery to the next paint.
  // Check below that these notifications stop once the layout settles.
  if (event.message === 'ResizeObserver loop completed with undelivered notifications.') window.resizeNotifications++;
  else window.testErrors.push(event.message);
});
window.addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
globalThis.WebSocket = class {
  constructor(url) {
    if (!String(url).includes('/chat/')) throw new Error('Unexpected socket: ' + url);
    queueMicrotask(() => this.onopen?.());
  }
  send(raw) {
    if (JSON.parse(raw).type === 'join') queueMicrotask(() => this.onmessage?.({data:JSON.stringify({type:'ready',agent:true})}));
  }
  close() { this.onclose?.(); }
};
globalThis.fetch = async (url) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Outline tester',providers:[]});
  if (path.endsWith('/config')) return Response.json({text_extensions:['.html','.md','.qmd','.typ','.tex','.bib'],asset_extensions:[],max_path:200});
  const documentMatch = path.match(/\\/api\\/documents\\/([^/]+)$/);
  if (documentMatch) return Response.json({title:'Outline browser',created_at:'outline-test',role:'editor',source_format:'html',docs_origin:location.origin,can_moderate:true,can_see_sharing:true,renderers:['markdown']});
  if (path.endsWith('/frame')) return Response.json({token:'outline-frame',until:9999999999});
  if (path.endsWith('/chat')) return Response.json({id:'outline-chat',token:'private'});
  if (path.includes('/comments')) return Response.json({comments:[]});
  return new Response('', {status:404});
};
const { default: Reader } = await import(${JSON.stringify(join(root, "web/src/components/Reader.svelte"))});
const { mount } = await import(${JSON.stringify(join(root, "web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);

let httpServer;
let tab;
try {
  await build({
    configFile: false,
    root: join(root, "web"),
    plugins: [
      tailwindcss(),
      svelte(),
      { name: "mock-room", enforce: "pre", resolveId(id) {
        if (id === "../lib/room.js" || id === "../room.js" || id.endsWith("/src/lib/room.js")) return room;
      } },
    ],
    build: {outDir: out, emptyOutDir: true, lib: {entry, formats: ["es"], fileName: () => "check.js"}},
    logLevel: "error",
  });
  httpServer = createServer((request, response) => {
    if (request.url?.startsWith("/docs/")) {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    if (request.url?.startsWith("/raw/")) {
      response.setHeader("content-type", "text/html");
      response.end('<!doctype html><html><body><h1>Rendered document</h1><p>Outline browser frame.</p></body></html>');
      return;
    }
    const file = join(out, request.url?.split("?")[0].slice(1) || "");
    try {
      response.setHeader("content-type", file.endsWith(".css") ? "text/css" : "text/javascript");
      response.end(readFileSync(file));
    } catch {
      response.statusCode = 404;
      response.end();
    }
  });
  await new Promise((resolve, reject) => {
    httpServer.once("error", reject);
    httpServer.listen(0, "127.0.0.1", resolve);
  });
  const port = httpServer.address().port;
  tab = await browser("chromium", join(temp, "profile"), 20000 + Math.floor(Math.random() * 1000));
  const url = `http://127.0.0.1:${port}/docs/outline`;
  await tab.resize(1280, 900);
  await tab.navigate(url);

  const flush = () => tab.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  const visible = (selector) => tab.evaluate(`(() => { const node = document.querySelector(${JSON.stringify(selector)}); return Boolean(node?.getClientRects().length && getComputedStyle(node).display !== 'none' && getComputedStyle(node).visibility !== 'hidden'); })()`);
  const click = async (selector) => { await tab.evaluate(`document.querySelector(${JSON.stringify(selector)})?.click()`); await flush(); };
  const mobileNav = (label) => `.mobile-pane-nav [aria-label=${JSON.stringify(label)}]`;
  const waitOutline = async () => until("document outline", () => tab.evaluate("Boolean(document.querySelector('.outline'))"), 10000);
  const waitHeading = async (title) => until(`outline heading ${title}`, () => tab.evaluate(`Boolean([...document.querySelectorAll('.outline .outline-heading')].find(node => node.textContent.trim().includes(${JSON.stringify(title)})))`), 5000);
  const openOutline = async () => {
    if (await visible('.outline')) return;
    const desktop = await visible('.sidebar-activity [aria-label="Outline"]');
    await click(desktop ? '.sidebar-activity [aria-label="Outline"]' : mobileNav("Outline"));
    await waitOutline();
  };
  const openFiles = async () => {
    const desktop = await visible('.sidebar-activity [aria-label="Files"]');
    await click(desktop ? '.sidebar-activity [aria-label="Files"]' : mobileNav("Files"));
  };
  const openFile = async (path) => {
    await openFiles();
    await until(`file ${path}`, () => tab.evaluate(`Boolean(document.querySelector('.explorer-row[title=${JSON.stringify(path)}]'))`), 5000);
    await click(`.explorer-row[title=${JSON.stringify(path)}]`);
    const marker = {"main.html":"HTML", "notes.md":"Markdown", "chapter.qmd":"Quarto", "paper.typ":"Typst", "paper.tex":"LaTeX", "refs.bib":"No outline here"}[path];
    await until(`source ${path}`, () => tab.evaluate(`window.editorState().text.includes(${JSON.stringify(marker)})`), 5000);
  };
  const clickHeading = async (title) => {
    await waitHeading(title);
    const before = await tab.evaluate("document.querySelector('.cm-scroller')?.scrollTop || 0");
    await tab.evaluate(`[...document.querySelectorAll('.outline .outline-heading')].find(node => node.textContent.trim().includes(${JSON.stringify(title)})).click()`);
    await until(`active heading ${title}`, () => tab.evaluate(`(() => {
      const selected = document.querySelector('.outline .outline-heading[aria-current="location"]');
      const active = document.querySelector('.cm-activeLine');
      return selected?.textContent.trim().includes(${JSON.stringify(title)}) && active?.textContent.includes(${JSON.stringify(title)});
    })()`), 5000);
    const after = await tab.evaluate("document.querySelector('.cm-scroller')?.scrollTop || 0");
    assert.ok(after > before || after > 20, `heading ${title} scrolls the source`);
    assert.equal(await tab.evaluate("document.activeElement?.closest('.cm-editor') !== null"), true, `heading ${title} focuses source`);
  };

  await until("Reader source", () => tab.evaluate("Boolean(document.querySelector('.cm-editor'))"), 10000);
  await openOutline();
  assert.equal(await tab.evaluate("document.querySelector('.outline')?.getAttribute('aria-label')"), "Document outline");
  assert.ok(await tab.evaluate("document.querySelector('.sidebar-activity [aria-label=Outline]')?.getAttribute('aria-label')"), "outline rail has an accessible label");
  assert.equal(await tab.evaluate("document.querySelector('.sidebar-activity [aria-label=Outline]').getAttribute('aria-pressed')"), "true");
  await tab.evaluate("document.querySelector('.sidebar-activity [aria-label=Outline]').focus()");
  await until("Outline tooltip", () => tab.evaluate("[...document.querySelectorAll('[role=tooltip]')].some(node => node.textContent.trim() === 'Outline' && node.getClientRects().length)"), 5000);
  await clickHeading("HTML heading");

  // Every supported source syntax gets the same user-visible jump behavior.
  for (const [path, title, child, next] of [
    ["notes.md", "Markdown heading", "Markdown subsection", "Markdown next"],
    ["chapter.qmd", "Quarto heading", "Quarto subsection", "Quarto next"],
    ["paper.typ", "Typst heading", "Typst subsection", "Typst next"],
    ["paper.tex", "LaTeX heading", "LaTeX subsection", "LaTeX next"],
  ]) {
    await openFile(path);
    await openOutline();
    await clickHeading(title);
    await clickHeading(child);
    assert.equal(await tab.evaluate(`document.querySelector('.outline .outline-heading[aria-current="location"]')?.textContent.trim()`), child);
    await clickHeading(next);
    assert.equal(await tab.evaluate(`document.querySelector('.outline .outline-heading[aria-current="location"]')?.textContent.trim()`), next, "active heading follows the next top-level heading");
    assert.equal(await tab.evaluate(`Boolean([...document.querySelectorAll('.outline .outline-heading[aria-current="location"]')].find(node => node.textContent.includes(${JSON.stringify(child)})))`), false, "a previous subsection is no longer active");
  }

  // Empty and unsupported files do not inherit headings from the previous file.
  await openFile("refs.bib");
  await openOutline();
  assert.equal(await tab.evaluate("document.querySelectorAll('.outline .outline-heading').length"), 0, "unsupported files have no outline entries");

  await openFile("notes.md");
  await openOutline();
  await tab.evaluate("window.roomControl.remoteAppend('notes.md', '# Remote heading')");
  await until("remote outline update", () => tab.evaluate("[...document.querySelectorAll('.outline .outline-heading')].some(node => node.textContent.includes('Remote heading'))"), 5000);
  await tab.insert("\n\n# Local heading\n");
  await until("local outline update", () => tab.evaluate("[...document.querySelectorAll('.outline .outline-heading')].some(node => node.textContent.includes('Local heading'))"), 5000);
  assert.equal(await tab.evaluate("window.roomSent.some(message => message.type === 'y-update')"), true, "local edits use the Yjs room");
  assert.match(await tab.evaluate("window.roomControl.snapshot('notes.md')"), /Local heading/);
  if (process.env.OUTLINE_SCREENSHOT) {
    const screenshot = await tab.command("Page.captureScreenshot", {format: "png"});
    writeFileSync(process.env.OUTLINE_SCREENSHOT, Buffer.from(screenshot.data, "base64"));
  }

  // A document-only desktop layout still exposes the outline and a heading
  // jump brings the source back when a person asks to inspect it.
  await tab.evaluate("localStorage.setItem('librepaper-layout', JSON.stringify('document'))");
  await tab.navigate(url);
  await until("document-only layout", () => tab.evaluate("document.querySelector('main.reader')?.classList.contains('no-preview') === false"), 10000);
  await until("hidden source", () => tab.evaluate("getComputedStyle(document.querySelector('.editorpane')).display === 'none'"), 5000);
  await openOutline();
  await clickHeading("HTML heading");
  assert.equal(await visible('.editorpane'), true, "outline jump reveals source from document-only layout");

  // On a phone the outline is a workspace tab, and choosing a heading reveals
  // the source tab instead of leaving the user in the sidebar.
  await tab.resize(390, 844);
  await flush();
  await openFile("notes.md");
  await click(mobileNav("Outline"));
  await waitOutline();
  await clickHeading("Markdown heading");
  assert.equal(await tab.evaluate("document.querySelector('main.reader')?.classList.contains('mobile-source')"), true, "mobile outline jump reveals source");
  assert.equal(await visible('.viewport'), false, "mobile source view hides the document");
  assert.equal(await visible('.editorpane'), true, "mobile source view shows the editor");

  await flush();
  const resizeNotifications = await tab.evaluate("window.resizeNotifications");
  for (let i = 0; i < 5; i++) await flush();
  assert.equal(await tab.evaluate("window.resizeNotifications"), resizeNotifications, "pane layout settles without a continuing resize loop");
  assert.equal(await tab.evaluate("document.documentElement.scrollWidth <= innerWidth && document.documentElement.scrollHeight <= innerHeight"), true, "mobile outline navigation keeps the page inside the viewport");
  assert.deepEqual(await tab.evaluate("window.testErrors"), []);
  console.log("outline-browser: five source formats, caret jumps, file switching, live Yjs updates, empty files, responsive reveal and accessibility passed");
} finally {
  await tab?.close();
  httpServer?.close();
  rmSync(temp, {recursive: true, force: true});
}
