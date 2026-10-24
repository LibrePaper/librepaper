// Real Reader phone landing test. The room is the only module replaced, and the
// document, project and role answers come from a small fetch mock. One bundle
// is built; each case is picked from the page query string.
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
const files = server.getMap("files"), paths = server.getMap("paths"), meta = server.getMap("meta");
if (caseName === "editor") {
  const id = "main"; const text = new LoroText(); text.insert(0, "<h1>Main</h1><p>Phone editor project</p>");
  files.setContainer(id, text); paths.set(id, "main.html"); meta.set("main", id);
  for (let i = 0; i < 3; i++) { const file = new LoroText(); file.insert(0, 'chapter ' + i); files.setContainer('chapter-' + i, file); paths.set('chapter-' + i, 'chapter-' + i + '.html'); }
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
const editorCase = caseName === 'editor' || caseName === 'empty';
const MAIN_TEXT = '<h1>Shared</h1><p>Read-only projection</p>';
globalThis.fetch = async (url, init = {}) => {
  const path = String(url);
  if (path.endsWith('/me')) return Response.json({name:'Phone Tester',handle:'phone-tester',provider:'github',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.html', '.md'], text_extensions: ['.html', '.md'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/paper') return Response.json({
    title:'Phone landing paper', created_at:'test', role: editorCase ? 'editor' : caseName,
    source_format: caseName === 'empty' ? '' : 'html', docs_origin: location.origin,
    can_moderate:false, can_see_sharing:false });
  if (path.endsWith('/project')) return Response.json({
    format:'html', main:'main.html',
    tree:{ main:'main.html', files:{ 'main.html':{ path:'main.html', kind:'text' } } },
    texts:{ 'main.html': MAIN_TEXT }, changes:[], project_digest:'digest-1' });
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
    if (req.url.split("?")[0] === "/docs/paper") {
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
  const base = `http://127.0.0.1:${serverHttp.address().port}/docs/paper`;
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
  await openCase('?case=editor', 390, 844);
  await landOnPreview('editor');
  assert.equal(await b.evaluate(phoneDocument), true, 'an editor phone opens on the preview');
  assert.equal(await visible('.viewport'), true, 'an editor phone shows the preview frame');
  assert.equal(await visible('.sidebar'), false, 'an editor phone does not show the files column');
  assert.equal(await visible('.editorpane'), false, 'an editor phone does not show the source');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the editor case reports no page errors');

  // Case 2: an editor project with no files shows the empty card, and Open Files leads to the files.
  const nothingToPreview = "Boolean([...document.querySelectorAll('section.latexpane .notyet h2')].find(node => node.textContent.trim() === 'Nothing to preview')?.getClientRects().length)";
  const openFilesButton = "[...document.querySelectorAll('section.latexpane .notyet button')].find(node => node.textContent.trim() === 'Open Files')";
  await openCase('?case=empty', 390, 844);
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

  // Case 3: a commenter link opens on the preview, and the comments column
  // takes the collaboration panel once the window is wide enough.
  await openCase('?case=commenter#k=commentkey', 390, 844);
  await landOnPreview('commenter');
  assert.equal(await b.evaluate(phoneDocument), true, 'a commenter phone opens on the preview');
  assert.equal(await visible('.sidebar'), false, 'a commenter phone does not open on the comments column');
  assert.equal(await visible('.editorpane'), false, 'a commenter phone does not open on the source');
  await b.resize(1280, 900); await flush();
  await until('commenter comments column', () => visible('.sidebar'), 5000);
  assert.equal(await visible('.sidebar'), true, 'a commenter window shows the comments column');
  assert.equal(await visible('.sidebar section.collaboration'), true, 'the comments column holds the collaboration panel rather than an empty column');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the commenter case reports no page errors');

  // Case 4: a reader link opens on the preview, and has no column at desktop width.
  await openCase('?case=reader#k=readerkey', 390, 844);
  await landOnPreview('reader');
  assert.equal(await b.evaluate(phoneDocument), true, 'a reader phone opens on the preview');
  assert.equal(await visible('.sidebar'), false, 'a reader phone does not open on the files column');
  assert.equal(await visible('.editorpane'), false, 'a reader phone does not open on the source');
  await b.resize(1280, 900); await flush();
  assert.equal(await visible('.sidebar'), false, 'a reader window has no column at desktop width');
  assert.deepEqual(await b.evaluate('window.testErrors'), [], 'the reader case reports no page errors');

  console.log('phone-landing-browser: editors, empty projects, commenter and reader links open on the preview passed');
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
