// Real Reader check of the two ways to move between the source and the preview
// with Keep in step off, and of the Keep in step toggle in the divider's pill.
// The room is the only module replaced, and the document and role answers come
// from a small fetch mock, as in preview-follows-browser.mjs. The preview frame
// is the real agent frame script (frame.js) on the documents origin; the
// document it shows is handed to it as a preview message, because the
// Markdown renderer needs a server this check does not run. Everything that
// moves the source or the preview is the Reader's own code: the pill, the
// caret, the reading-position round trip and the double-click in the frame.
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
const temp = mkdtempSync(join(tmpdir(), "librepaper-show-in-panes-"));
const entry = join(temp, "entry.js");
const room = join(temp, "room.js");
const out = join(temp, "build");

// Forty sections, each a heading and one paragraph with a number of its own.
// Section N's heading is on line 3 + 4(N-1) and its paragraph on line 5 + 4(N-1).
const sections = 40;
const reportLines = ["# Report", ""];
for (let n = 1; n <= sections; n++) reportLines.push(`## Section ${n}`, "", `Paragraph ${n} says something unique number ${n}.`, "");
const report = reportLines.join("\n");
const sourcepos = (line) => `data-sourcepos="${line}:1-${line}:${reportLines[line - 1].length}"`;
const previewPage = `<!doctype html><html><head><meta charset="utf-8"><title>Report</title></head><body><h1 ${sourcepos(1)}>Report</h1>${
  Array.from({ length: sections }, (_, index) => {
    const n = index + 1;
    const heading = 3 + 4 * index;
    return `<h2 ${sourcepos(heading)}>Section ${n}</h2><p ${sourcepos(heading + 2)}>Paragraph ${n} says something unique number ${n}.</p>`;
  }).join("")
}</body></html>`;

writeFileSync(room, `
import { LoroDoc, LoroText } from "loro-crdt";
const encode = b => btoa(String.fromCharCode(...b));
const server = new LoroDoc();
const files = server.getMap("files"), paths = server.getMap("paths");
const project = {
  "report.md": ${JSON.stringify(report)},
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

// The CodeMirror view class is exposed so the check can read and move the
// source caret. It comes from the same module the Reader uses, by absolute path.
writeFileSync(entry, `
import ${JSON.stringify(join(root,"web/src/styles/app.css"))};
import { EditorView } from ${JSON.stringify(join(root,"web/node_modules/@codemirror/view/dist/index.js"))};
window.EditorView = EditorView;
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
  if (path.endsWith('/me')) return Response.json({name:'Panes Tester',handle:'panes-tester',provider:'github',providers:[]});
  if (path.endsWith('/config')) return Response.json({ extensions: ['.md'], text_extensions: ['.md'], asset_extensions: ['.png'], derived_extensions: [] });
  if (path === '/api/documents/' + location.pathname.split('/').pop()) return Response.json({
    title:'Show in panes', created_at:'test', role:'editor', source_format:'markdown', docs_origin: location.origin,
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
    const pathname = req.url.split("?")[0];
    // The Reader's own page is /docs/<slug>; its preview frame is /raw/<slug>/.
    if (pathname.startsWith("/raw/") && pathname.endsWith("/")) {
      res.setHeader("content-type", "text/html");
      res.end(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><script src="/frame.js?reader=${encodeURIComponent(`http://${req.headers.host}`)}"></script></body></html>`);
      return;
    }
    if (pathname.startsWith("/docs/")) {
      res.setHeader("content-type", "text/html");
      res.end('<!doctype html><html data-theme="librepaper"><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/librepaper-web.css"></head><body><script type="module" src="/check.js"></script></body></html>');
      return;
    }
    // The preview frame's script is the one the app ships, from web/dist.
    if (pathname === "/frame.js") {
      res.setHeader("content-type", "text/javascript");
      res.end(readFileSync(join(root, "web/dist/frame.js")));
      return;
    }
    const file = join(out, pathname.slice(1));
    try { res.setHeader("content-type", contentType(file)); res.end(readFileSync(file)); }
    catch { res.statusCode = 404; res.end(); }
  });
  await new Promise((resolve, reject) => { serverHttp.once("error", reject); serverHttp.listen(0, "127.0.0.1", resolve); });
  b = await browser("chromium", join(temp, "profile"), 19000 + Math.floor(Math.random() * 1000));
  await b.resize(1400, 900);
  await b.navigate(`http://127.0.0.1:${serverHttp.address().port}/docs/show-in-panes`);

  const flush = () => b.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
  const click = async (selector) => { await b.evaluate(`(() => { const node=document.querySelector(${JSON.stringify(selector)}); node?.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerType:'mouse'})); node?.click(); })()`); await flush(); };
  const exists = (selector) => b.evaluate(`Boolean(document.querySelector(${JSON.stringify(selector)}))`);
  const pillButtons = () => b.evaluate(`[...document.querySelectorAll('.grip-pill .grip-pill-button')].map(node => ({ label: node.getAttribute('aria-label'), pressed: node.getAttribute('aria-pressed') }))`);

  // The preview frame's document, read from the Reader's page. Every helper
  // gives null while the frame is not there yet.
  const frame = (body) => `(() => { const f = document.querySelector('iframe[title=Document]'); const w = f?.contentWindow; const d = f?.contentDocument; if (!w || !d) return null; ${body} })()`;
  const paragraphOf = (n) => `[...d.querySelectorAll('p')].find(node => node.textContent.includes(${JSON.stringify(`Paragraph ${n} says`)}))`;
  const frameScroll = () => b.evaluate(frame('return w.scrollY;'));
  const scrollFrame = (top) => b.evaluate(frame(`w.scrollTo({ top: ${top}, behavior: "instant" }); return true;`));
  const paragraphTop = (n) => b.evaluate(frame(`const p = ${paragraphOf(n)}; return p ? p.getBoundingClientRect().top + w.scrollY : null;`));
  const paragraphInView = (n) => b.evaluate(frame(`const p = ${paragraphOf(n)}; if (!p) return false; const r = p.getBoundingClientRect(); return r.top >= 0 && r.bottom <= w.innerHeight;`));
  const frameHasParagraph = (n) => b.evaluate(frame(`return Boolean(${paragraphOf(n)});`));
  // Hands the preview document to the frame. The frame only takes messages
  // from its parent, which is this page, so the message comes from here.
  const feedPreview = () => b.evaluate(frame(`w.postMessage({ librepaper: true, type: "preview", html: ${JSON.stringify(previewPage)}, sources: ["report.md"] }, "*"); return true;`));

  // The source caret, through the CodeMirror view of the source editor.
  const setCaret = (at) => b.evaluate(`(() => { const view = window.EditorView.findFromDOM(document.querySelector('.cm-editor')); view.focus(); view.dispatch({ selection: { anchor: ${at} } }); return true; })()`);
  const caretLine = () => b.evaluate(`(() => { const view = window.EditorView.findFromDOM(document.querySelector('.cm-editor')); return view.state.doc.lineAt(view.state.selection.main.head).text; })()`);
  const sourceOffset = (needle) => report.indexOf(needle) + 3;

  // 1. Keep in step is on by default: the pill holds one lock, pressed.
  await until("the divider pill", () => exists(".grip-pill .grip-pill-button"), 15000);
  const firstPill = await pillButtons();
  assert.equal(firstPill.length, 1, `with Keep in step on by default the pill shows exactly one button, it shows ${firstPill.length}`);
  assert.equal(firstPill[0].pressed, "true", "with Keep in step on by default the one pill button is pressed");
  assert.equal(await exists(".grip-pill .grip-pill-button.is-on"), true, "the one pill button is the lock that is on");

  // 2. The lock turns Keep in step off: the two direction buttons and the
  // unlock toggle appear, and the choice is stored.
  await click(".grip-pill .grip-pill-button.is-on");
  await until("the direction buttons once Keep in step is off", async () => (await pillButtons()).length === 3, 5000);
  const offPill = await pillButtons();
  assert.deepEqual(offPill.map(one => one.label), ["Show in preview", "Show in source", "Keep in step"], "with Keep in step off the pill shows the two directions and the toggle");
  assert.equal(offPill[2].pressed, "false", "the toggle reads aria-pressed=false with Keep in step off");
  assert.equal(await b.evaluate('localStorage.getItem("librepaper-linked")'), "false", "turning Keep in step off stores false under librepaper-linked");

  // The preview gets its document once its frame is there.
  await until("the preview frame", () => exists('iframe[title=Document]'), 15000);
  await until("the preview holds the report", async () => {
    if (await frameHasParagraph(30)) return true;
    await feedPreview();
    return false;
  }, 15000);

  // 3. Show in preview: from the top of the preview, the source caret deep in
  // the document brings the preview down to it.
  await until("the preview at the top", async () => {
    await scrollFrame(0);
    return (await frameScroll()) === 0;
  }, 5000);
  const scrolledBefore = await frameScroll();
  await setCaret(sourceOffset("Paragraph 30 says"));
  await flush();
  await click('.grip-pill-button[aria-label="Show in preview"]');
  await until("the preview scrolls down", async () => (await frameScroll()) > scrolledBefore, 10000);
  assert.ok((await frameScroll()) > scrolledBefore, "Show in preview scrolls the preview down to the source caret");
  await until("Paragraph 30 is in the preview's viewport", () => paragraphInView(30), 10000);
  assert.equal(await paragraphInView(30), true, "Show in preview leaves Paragraph 30 inside the preview's viewport");

  // 4. Show in source: the preview's reading place goes to the source caret,
  // with Keep in step still off.
  await scrollFrame((await paragraphTop(10)) - 40);
  await flush();
  await setCaret((await b.evaluate('window.EditorView.findFromDOM(document.querySelector(".cm-editor")).state.doc.length')));
  await flush();
  await click('.grip-pill-button[aria-label="Show in source"]');
  await until("the source caret at the passage near the top of the preview", async () => /\b(Section|Paragraph) (9|10|11)\b/.test(await caretLine()), 10000);
  assert.match(await caretLine(), /\b(Section|Paragraph) (9|10|11)\b/, "Show in source moves the source caret to the passage at the top of the preview (Section or Paragraph 10, within one)");

  // 5. A double-click on a passage in the preview moves the source caret there,
  // with Keep in step still off.
  await setCaret(0);
  await flush();
  await b.evaluate(frame(`const p = ${paragraphOf(20)}; p.scrollIntoView({ block: "center", behavior: "instant" }); return true;`));
  await flush();
  // The frame reads where a double-click landed from its clientX/clientY,
  // through caretPositionFromPoint, so the event is dispatched in the frame at
  // the middle of the paragraph's first line.
  const fired = await b.evaluate(frame(`const p = ${paragraphOf(20)}; const r = p.getClientRects()[0]; const x = r.left + Math.min(40, r.width / 2), y = r.top + r.height / 2; p.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, clientX: x, clientY: y, detail: 2 })); return true;`));
  assert.ok(fired, "Paragraph 20 has a place on the page to double-click");
  await until("the source caret on Paragraph 20", async () => (await caretLine()).includes("Paragraph 20 says"), 10000);
  assert.ok((await caretLine()).includes("Paragraph 20 says"), "a double-click in the preview moves the source caret to that passage with Keep in step off");
  assert.equal(await b.evaluate('localStorage.getItem("librepaper-linked")'), "false", "Keep in step is still off after the double-click");

  // 6. The unlock toggle turns Keep in step back on: one lock again, pressed.
  await click('.grip-pill-button[aria-pressed="false"]');
  await until("the single lock once Keep in step is back on", async () => {
    const pill = await pillButtons();
    return pill.length === 1 && pill[0].pressed === "true";
  }, 5000);
  assert.equal(await b.evaluate('localStorage.getItem("librepaper-linked")'), "true", "turning Keep in step back on stores true");
  assert.deepEqual(await b.evaluate('window.testErrors'), [], "the page reports no errors");

  console.log("show-in-panes-browser: Keep in step is on by default, its toggle sets the pill, Show in preview and Show in source move the panes with it off, a double-click moves the source, and the lock restores");
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
