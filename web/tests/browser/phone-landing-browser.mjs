// Real Reader phone landing test. The room is the only module replaced, and the
// document, project and role answers come from a small fetch mock. One bundle
// is built; each case is picked from the page query string and opens its own
// slug, so the copy a browser keeps of one project never stands in for another.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-phone-landing-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");

// The room is read only by editors. The case is chosen from the query string
// at runtime, so one bundle serves every case.
writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const caseName = new URLSearchParams(location.search).get("case") || "editor";
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths");
if (caseName === "editor") {
  const id = "main"; const text = new LoroText(); text.insert(0, "<h1>Main</h1><p>Phone editor project</p>");
  files.setContainer(id, text); paths.set(id, "main.html");
  for (let i = 0; i < 3; i++) { const file = new LoroText(); file.insert(0, 'chapter ' + i); files.setContainer('chapter-' + i, file); paths.set('chapter-' + i, 'chapter-' + i + '.html'); }
}
// A project whose only file has no renderer: nothing for the preview to draw.
if (caseName === "bibonly") {
  const text = new LoroText(); text.insert(0, "@book{a, title={A}}");
  files.setContainer("refs", text); paths.set("refs", "refs.bib");
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
// Live chat opens a socket that this test never needs. A socket that never
// opens keeps the chat idle without a real server behind it.
globalThis.WebSocket = class {
  constructor() {}
  send() {}
  close() {}
};
const caseName = new URLSearchParams(location.search).get('case') || 'editor';
const editorCase = ['editor', 'empty', 'bibonly'].includes(caseName);
const MAIN_TEXT = '<h1>Shared</h1><p>Read-only projection</p>';
const NOTES_TEXT = 'key = value';
globalThis.fetch = async (url, init = {}) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Phone Tester',handle:'phone-tester',provider:'github',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md', '.bib'], text_extensions: ['.html', '.md', '.bib'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/' + location.pathname.split('/').pop()) return Response.json({
    title:'Phone landing paper', created_at:'test', role: editorCase ? 'editor' : caseName,
    source_format: ['empty', 'bibonly'].includes(caseName) ? '' : 'html', docs_origin: location.origin,
    can_moderate:false, can_see_sharing:false });
  if (path.endsWith('/project')) return Response.json({
    format:'html',
    tree:{ files:{ 'main.html':{ path:'main.html', kind:'text' }, 'notes.ini':{ path:'notes.ini', kind:'text' } } },
    texts:{ 'main.html': MAIN_TEXT, 'notes.ini': NOTES_TEXT }, changes:[], project_digest:'digest-1' });
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
    if (req.url.startsWith('/raw/')) {
      res.setHeader('content-type', 'text/html');
      res.end('<body style="height:4000px">A long document</body>');
      return;
    }
    const file = join(out, req.url.split("?")[0].slice(1));
    try { res.setHeader("content-type", contentType(file)); res.end(readFileSync(file)); }
    catch { res.statusCode = 404; res.end(); }
  });
  await new Promise((resolve, reject) => { serverHttp.once("error", reject); serverHttp.listen(0, "127.0.0.1", resolve); });
  b = await browser("chromium", join(temp, "profile"), 19000 + Math.floor(Math.random() * 1000));
  const base = `http://127.0.0.1:${serverHttp.address().port}/docs/`;
  const flush = () => b.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const visible = (selector) => b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); return Boolean(node?.getClientRects().length && getComputedStyle(node).visibility !== 'hidden'); })()`);
  const phoneDocument = "Boolean(document.querySelector('main.reader.mobile-document'))";
  const openCase = async (query, width, height) => {
    await b.resize(width, height);
    await b.navigate(base + query);
  };
  // A fresh load at the phone size: the Reader lands on the preview.
  const landOnPreview = async (label) => {
    await until(`${label} landing`, () => b.evaluate(phoneDocument), 10000);
    await until(`${label} preview frame shown`, () => visible('.viewport'), 10000);
    await flush();
  };

  // Case 1: a normal editor project opens on the preview.
  await openCase('editor?case=editor', 390, 844);
  await landOnPreview('editor');
  assert.equal(await b.evaluate(phoneDocument), true, 'an editor phone opens on the preview');
  assert.equal(await visible('.viewport'), true, 'an editor phone shows the preview frame');
  assert.equal(await visible('.sidebar'), false, 'an editor phone does not show the files column');
  assert.equal(await visible('.editorpane'), false, 'an editor phone does not show the source');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the editor case reports no page errors');

  // A tapped file on a phone opens on the preview. The Files column is opened
  // from the Menu, as a person does it, and chapter-1.html is tapped there.
  const press = async (selector) => { await b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); node.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse'})); node.click(); })()`); await flush(); };
  await press('.compact-panels-trigger[aria-label="Menu"]');
  await until('menu lists the files panel', () => b.evaluate(`Boolean(document.querySelector('.explorer-menu[data-state="open"] [data-panel-id="files"]'))`), 5000);
  await press('.explorer-menu[data-state="open"] [data-panel-id="files"]');
  await until('files column shows the files', () => visible('.sidebar .explorer'), 5000);
  await until('chapter-1.html is listed', () => b.evaluate(`Boolean(document.querySelector('.explorer-row[title="chapter-1.html"]'))`), 5000);
  await b.evaluate(`document.querySelector('.explorer-row[title="chapter-1.html"]').click()`);
  await until('a tapped file opens on the preview', async () => !(await visible('.sidebar')) && await b.evaluate(phoneDocument), 5000);
  await until('a tapped file shows the preview frame', () => visible('.viewport'), 5000);
  await flush();
  assert.equal(await b.evaluate(phoneDocument), true, 'a tapped file on a phone opens on the preview');
  assert.equal(await visible('.viewport'), true, 'a tapped file on a phone shows the preview frame');
  assert.equal(await visible('.editorpane'), false, 'a tapped file on a phone does not show the source');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the tapped file case reports no page errors');

  // Case 2: an editor project with no files shows the empty card, and Open Files leads to the files.
  const nothingToPreview = "Boolean([...document.querySelectorAll('section.latexpane .notyet h2')].find(node => node.textContent.trim() === 'Nothing to preview')?.getClientRects().length)";
  const openFilesButton = "[...document.querySelectorAll('section.latexpane .notyet button')].find(node => node.textContent.trim() === 'Open Files')";
  await openCase('empty?case=empty', 390, 844);
  await until('empty landing', () => b.evaluate(phoneDocument), 10000);
  await until('empty project card in the preview', () => b.evaluate(nothingToPreview), 10000);
  await flush();
  assert.equal(await b.evaluate(phoneDocument), true, 'an empty project phone opens on the preview');
  assert.equal(await b.evaluate(nothingToPreview), true, 'an empty project shows the Nothing to preview card');
  assert.equal(await b.evaluate(`Boolean(${openFilesButton})`), true, 'the empty card has an Open Files button');
  await b.evaluate(`${openFilesButton}.click()`);
  await until('Open Files shows the files column', () => b.evaluate("document.querySelector('main.reader')?.classList.contains('mobile-sidebar')"), 5000);
  await flush();
  assert.equal(await visible('.sidebar'), true, 'Open Files shows the files column');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the empty project case reports no page errors');

  // A project whose only file has no renderer says so in the preview, and the
  // card points at Files rather than offering a lock there is nothing to lock.
  await openCase('bibonly?case=bibonly', 390, 844);
  await until('bib-only landing', () => b.evaluate(phoneDocument), 10000);
  await until('bib-only card in the preview', () => b.evaluate(nothingToPreview), 10000);
  const cardText = await b.evaluate("[...document.querySelectorAll('section.latexpane .notyet p')].map(node => node.textContent.trim())");
  assert.ok(cardText.includes('refs.bib has no preview of its own.'),
    `the card names the file and says it has no preview of its own: ${JSON.stringify(cardText)}`);
  assert.equal(
    await b.evaluate("[...document.querySelectorAll('section.latexpane .notyet p')].some(node => node.textContent.includes('Lock the preview on a file with the eye in Files'))"),
    true, 'and says how to keep a page here');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the bib-only case reports no page errors');

  // Case 3: a commenter link opens on the preview, and the column opens on
  // the files once the window is wide enough.
  await openCase('commenter?case=commenter#k=commentkey', 390, 844);
  await landOnPreview('commenter');
  assert.equal(await b.evaluate(phoneDocument), true, 'a commenter phone opens on the preview');
  assert.equal(await visible('.sidebar'), false, 'a commenter phone does not open on the comments column');
  assert.equal(await visible('.editorpane'), false, 'a commenter phone does not open on the source');
  await b.resize(1280, 900); await flush();
  await until('commenter comments column', () => visible('.sidebar:not(.collapsed)'), 5000);
  assert.equal(await visible('.sidebar:not(.collapsed)'), true, 'a commenter window shows the comments column');
  assert.equal(await visible('.sidebar .explorer'), true, 'the commenter column holds the files rather than an empty column');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the commenter case reports no page errors');

  // Case 4: a reader link opens on the preview on a phone, and at desktop
  // width the column opens on the project's files, read only.
  await openCase('reader?case=reader#k=readerkey', 390, 844);
  await landOnPreview('reader');
  assert.equal(await b.evaluate(phoneDocument), true, 'a reader phone opens on the preview');
  assert.equal(await visible('.sidebar'), false, 'a reader phone does not open on the files column');
  assert.equal(await visible('.editorpane'), false, 'a reader phone does not open on the source');
  await b.resize(1280, 900); await flush();
  await until('reader files column', () => visible('.sidebar:not(.collapsed) .explorer'), 5000);
  assert.equal(await b.evaluate(`Boolean(document.querySelector('.sidebar-activity [aria-label="Files"]'))`), true, 'a reader is offered the Files panel');
  assert.deepEqual(await b.evaluate(`[...document.querySelectorAll('.explorer-row')].map((node) => node.getAttribute('title'))`), ['main.html', 'notes.ini'], 'the reader sees every file of the project');
  assert.equal(await b.evaluate(`Boolean(document.querySelector('.explorer [aria-label="New file"], .explorer [aria-label="New folder"], .explorer [aria-label="Upload files"], .explorer input[type="file"]'))`), false, 'the reader list has no add or upload controls');
  // A text that does not render is shown as text, where the preview was, and
  // the editor is never loaded for it.
  await b.evaluate(`document.querySelector('.explorer-row[title="notes.ini"]').click()`);
  await until('reader file text', () => visible('.fileview-text'), 5000);
  assert.equal(await b.evaluate(`document.querySelector('.fileview-text').textContent`), 'key = value', 'the reader sees the file text read only');
  assert.equal(await visible('.viewport'), false, 'the preview is hidden while the file is shown');
  assert.equal(await b.evaluate(`Boolean(document.querySelector('.cm-editor, #reader-source'))`), false, 'no editor is loaded for a reader');
  const backToPreview = `[...document.querySelectorAll('.fileview button')].find((node) => node.textContent.trim() === 'Back to preview')`;
  await b.evaluate(`${backToPreview}.click()`);
  await until('reader preview returns', () => visible('.viewport'), 5000);
  assert.equal(await b.evaluate(`Boolean(document.querySelector('.fileview'))`), false, 'the back button returns to the preview');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the reader case reports no page errors');

  console.log('phone-landing-browser: editors, empty and bib-only projects, commenter and reader links open on the preview passed; link holders list the files and read any of them');
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
