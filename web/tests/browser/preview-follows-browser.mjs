// Real Reader check that the preview follows the file open in the source.
// The room is the only module replaced, and the document and role answers come
// from a small fetch mock, as in phone-landing-browser.mjs. The project has a
// document (report.md) that includes a chapter, a standalone file
// (summary.md) and a bibliography that no renderer draws.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

const root = dirname(dirname(dirname(dirname(fileURLToPath(import.meta.url)))));
const temp = mkdtempSync(join(tmpdir(), "librepaper-preview-follows-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");

writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths");
const project = {
  "report.md": "# Report\\n\\n{{< include chapters/intro.md >}}\\n",
  "chapters/intro.md": "## Introduction\\n\\nWhy this matters.\\n",
  "summary.md": "# Summary\\n\\nA page of its own.\\n",
  "refs.bib": "@book{a, title={A}}\\n",
};
for (const [path, body] of Object.entries(project)) {
  const text = new LoroText(); text.insert(0, body);
  files.setContainer(path, text); paths.set(path, path);
}
server.commit();
const update = encode(server.export({ mode: "update" }));
const vector = encode(server.oplogVersion().encode());
export function openRoom(slug, {onMessage, onConnected}) {
  window.roomReceive = onMessage;
  window.roomSent = [];
  queueMicrotask(() => onConnected(true));
  return {
    send(message) {
      window.roomSent.push(message);
      if (message.type === "doc-open") queueMicrotask(() => {
        onMessage({type:"doc-state", protocol:"librepaper.room.v3", vector, durableVector: vector, updates:[update]});
        onMessage({type:"doc-peers", count:1});
      });
      return {ok:true};
    },
    sendLive(message) { window.roomSent.push(message); return {ok:true}; },
    close() {}
  };
}
`);

writeFileSync(entry, `
import ${JSON.stringify(join(root,"web/src/styles/app.css"))};
window.testErrors = [];
window.addEventListener('error', event => window.testErrors.push(event.message));
window.addEventListener('unhandledrejection', event => window.testErrors.push(String(event.reason)));
// Live chat opens a socket that this test never needs.
globalThis.WebSocket = class {
  constructor() {}
  send() {}
  close() {}
};
globalThis.fetch = async (url, init = {}) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Follow Tester',handle:'follow-tester',provider:'github',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.md', '.bib'], text_extensions: ['.md', '.bib'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/' + location.pathname.split('/').pop()) return Response.json({
    title:'Preview follows paper', created_at:'test', role:'editor', source_format:'markdown', docs_origin: location.origin,
    can_moderate:false, can_see_sharing:false });
  if (path.endsWith('/frame')) return Response.json({token:'frame-token',until:9999999999});
  if (path.includes('/comments')) return Response.json({comments:[]});
  return { ok:false, status:404, json: async () => ({}) };
};
const { default: Reader } = await import(${JSON.stringify(join(root,"web/src/components/Reader.svelte"))});
const { mount } = await import(${JSON.stringify(join(root,"web/node_modules/svelte/src/index-client.js"))});
mount(Reader, {target:document.body});
`);

let serverHttp, b;
try {
  await build({ configFile:false, define: KATEX_DEFINE, root:join(root,"web"), resolve:{alias:loroAlias}, plugins:[tailwindcss(), svelte(), {name:"mock-room", enforce:"pre", resolveId(id){ if(id === '../lib/room.js' || id === '../room.js' || id.endsWith('/src/lib/room.js')) return room; }}], build:{outDir:out,emptyOutDir:true,lib:{entry,formats:["es"],fileName:()=>"check.js"}}, logLevel:"error" });
  serverHttp = createServer((req, res) => {
    if (serveKatex(req, res)) return;
    if (req.url.startsWith("/docs/")) {
      res.setHeader("content-type", "text/html");
      res.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    const file = join(out, req.url.split("?")[0].slice(1));
    try { res.setHeader("content-type", contentType(file)); res.end(readFileSync(file)); }
    catch { res.statusCode = 404; res.end(); }
  });
  await new Promise((resolve, reject) => { serverHttp.once("error", reject); serverHttp.listen(0, "127.0.0.1", resolve); });
  b = await browser("chromium", join(temp, "profile"), 19000 + Math.floor(Math.random() * 1000));
  await b.resize(1280, 900);
  await b.navigate(`http://127.0.0.1:${serverHttp.address().port}/docs/follows`);

  const flush = () => b.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const click = async (selector) => { await b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); node?.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse'})); node?.click(); })()`); await flush(); };
  const row = (path) => `.explorer-row[title=${JSON.stringify(path)}]`;
  const exists = (selector) => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(selector)}))`);
  const chip = () => b.evaluate(`document.querySelector('.notice-chip')?.textContent.replace(/\\s+/g, ' ').trim() || ''`);
  const shownRow = () => b.evaluate(`document.querySelector('.explorer-row [aria-label="Shown in the preview"]')?.closest('.explorer-row')?.getAttribute('title') || ''`);
  const cardText = () => b.evaluate(`[...document.querySelectorAll('section.latexpane .notyet p')].map(node => node.textContent.replace(/\\s+/g, ' ').trim()).join(' | ')`);
  const openFile = async (path) => {
    await until(`file ${path}`, () => exists(row(path)), 10000);
    await click(row(path));
  };

  // The project opens on the first file that renders on its own and that
  // nothing includes, and the preview shows it.
  await until("the files list", () => exists(row("report.md")), 15000);
  await until("the preview on report.md", async () => (await shownRow()) === "report.md", 10000);
  assert.equal(await chip(), "", "a file shown as itself has no chip");

  // Opening a file that renders on its own moves the preview.
  await openFile("summary.md");
  await until("the preview follows summary.md", async () => (await shownRow()) === "summary.md", 10000);
  assert.equal(await chip(), "", "no chip while the preview shows the open file");

  // An included file is shown inside the file that includes it.
  await click(row("chapters"));
  await openFile("chapters/intro.md");
  await until("the shown-inside chip", async () => (await chip()) === "intro.md, shown inside report.md", 10000);
  assert.equal(await shownRow(), "report.md", "the eye marks the file the preview shows");

  // A file nothing renders gets a card instead of a page.
  await openFile("refs.bib");
  await until("the card for refs.bib", async () => (await cardText()).includes("refs.bib has no preview of its own."), 10000);
  assert.ok((await cardText()).includes("Lock the preview on a file with the eye in Files"), "the card says how to keep a page here");
  assert.equal(await chip(), "", "the card has no chip");

  // The eye locks the preview without opening the file.
  await click(`${row("summary.md")} button[aria-label="Lock preview on summary.md"]`);
  await until("the lock chip", async () => (await chip()).startsWith("Locked: summary.md"), 10000);
  assert.equal(await exists(`${row("summary.md")} button[aria-label="Unlock the preview"]`), true, "the locked row shows the solid eye");
  assert.equal(await b.evaluate(`document.querySelector('.explorer-row[data-selected]')?.getAttribute('title')`), "refs.bib", "locking leaves the open file alone");

  // The source can move and the preview stays.
  await openFile("chapters/intro.md");
  await flush();
  assert.ok((await chip()).startsWith("Locked: summary.md"), "the preview stays on the locked file");

  // Closing the chip unlocks, and the preview follows the open file again.
  await click('.notice-chip button[aria-label="Unlock the preview"]');
  await until("the shown-inside chip again", async () => (await chip()) === "intro.md, shown inside report.md", 10000);
  assert.equal(await exists('button[aria-label="Unlock the preview"]'), false, "nothing is locked any more");
  assert.deepEqual(await b.evaluate('window.testErrors'), [], "the page reports no errors");

  console.log("preview-follows-browser: following, shown inside, no preview card, lock and unlock passed");
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
