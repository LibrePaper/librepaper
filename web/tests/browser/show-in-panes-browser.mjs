// Real Reader check of the two ways to move between the source and the preview
// with Keep in step off, and of the Keep in step toggle in the divider's pill.
// The room is the only module replaced, and the document and role answers come
// from a small fetch mock, as in preview-follows-browser.mjs. The preview frame
// is the real agent frame script (frame.js), and the document in it is the
// Reader's own render of report.md by the Markdown module from web/wasm.
// Everything that moves the source or the preview is the Reader's own code:
// the pill, the caret, the reading-position round trip, and a double-click
// sent through the browser's pointer.
import assert from "node:assert/strict";
import { build } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { createServer } from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { browser, until, removeTemporary } from "../helpers/browser-driver.mjs";
import { assertFreshDist } from "../helpers/fresh-dist.mjs";
import { contentType, loroAlias } from "../helpers/loro.mjs";
import { KATEX_DEFINE, serveKatex } from "../helpers/katex.mjs";

// This check drives the built frame script, so a stale web/dist would test old code.
assertFreshDist("show-in-panes-browser");

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

// The preview is the Reader's own render of report.md, drawn by the Markdown
// module from web/wasm. The renderer checks a module against the digest in its
// URL, as the mirror lays it out.
const markdownWasm = join(root, "web/wasm/markdown.wasm");
if (!existsSync(markdownWasm)) {
  console.log("show-in-panes-browser: no markdown module in web/wasm; skipping (fetch the wasm modules first)");
  process.exit(0);
}
const markdownBytes = readFileSync(markdownWasm);
const modules = { markdown: `/wasm/${createHash("sha256").update(markdownBytes).digest("hex")}/markdown.wasm` };

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
globalThis.LIBREPAPER_MODULES = ${JSON.stringify(modules)};
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
    if (req.url === modules.markdown) { res.setHeader("content-type", "application/wasm"); res.end(markdownBytes); return; }
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
  const pageUrl = `http://127.0.0.1:${serverHttp.address().port}/docs/show-in-panes`;
  await b.navigate(pageUrl);

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

  // The source caret, through the CodeMirror view of the source editor.
  const setCaret = (at) => b.evaluate(`(() => { const view = window.EditorView.findFromDOM(document.querySelector('.cm-editor')); view.focus(); view.dispatch({ selection: { anchor: ${at} } }); return true; })()`);
  const caretLine = () => b.evaluate(`(() => { const view = window.EditorView.findFromDOM(document.querySelector('.cm-editor')); return view.state.doc.lineAt(view.state.selection.main.head).text; })()`);
  const sourceOffset = (needle) => report.indexOf(needle) + 3;
  // Whether the preview's landing highlight is on, or null while the frame is not there.
  const landingHighlight = () => b.evaluate(frame('return w.CSS.highlights.has("librepaper-landing");'));
  // The divider pill's centre, and the edges of the two panes it sits between:
  // the source pane and the preview pane.
  const pillCentre = () => b.evaluate(`(() => { const r = document.querySelector('.grip-pill')?.getBoundingClientRect(); return r ? r.left + r.width / 2 : null; })()`);
  const paneEdges = () => b.evaluate(`(() => { const e = document.querySelector('.editorpane')?.getBoundingClientRect(); const v = document.querySelector('.viewport:not(.away)')?.getBoundingClientRect(); return e && v ? { editor: { left: e.left, right: e.right }, viewport: { left: v.left, right: v.right } } : null; })()`);
  const assertPillBetweenPanes = async (side) => {
    await flush();
    const panes = await paneEdges();
    const x = await pillCentre();
    assert.ok(panes && x !== null, `with the source on the ${side} the panes and the divider pill are on the page`);
    const left = side === "right" ? panes.viewport : panes.editor;
    const right = side === "right" ? panes.editor : panes.viewport;
    assert.ok(x >= left.right - 20 && x <= right.left + 20, `with the source on the ${side} the divider pill sits between the panes (pill at ${x}, left pane ends at ${left.right}, right pane starts at ${right.left})`);
  };
  // Stores the source side the Reader reads on load, then reloads the page to apply it.
  const setSourceSide = async (side) => {
    await b.evaluate(`localStorage.setItem("librepaper-source-side", ${JSON.stringify(JSON.stringify(side))})`);
    await b.navigate(pageUrl);
    await until("the divider pill after a reload", () => exists(".grip-pill .grip-pill-button"), 15000);
    await until(`the source on the ${side}`, () => exists(side === "right" ? "main.reader.source-right" : "main.reader:not(.source-right)"), 15000);
  };

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

  // The Reader renders report.md into its preview frame, which is on screen.
  await until("the preview frame", () => exists('iframe[title=Document]'), 15000);
  await until("the preview holds the report", () => frameHasParagraph(30), 30000);
  assert.ok(await b.evaluate(`(() => { const r = document.querySelector('iframe[title=Document]').getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth && r.width > 200; })()`), "the preview frame is on screen beside the source");

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
  // The preview tints the passage it landed on, and the tint clears after about a second.
  await until("the preview landing highlight", async () => (await landingHighlight()) === true, 3000);
  await until("the preview landing highlight cleared", async () => (await landingHighlight()) === false, 3000);

  // 4. Show in source: the preview's reading place goes to the source caret,
  // with Keep in step still off.
  await scrollFrame((await paragraphTop(10)) - 40);
  await flush();
  await setCaret((await b.evaluate('window.EditorView.findFromDOM(document.querySelector(".cm-editor")).state.doc.length')));
  await flush();
  await click('.grip-pill-button[aria-label="Show in source"]');
  await until("the source caret at the passage near the top of the preview", async () => /\b(Section|Paragraph) (9|10|11)\b/.test(await caretLine()), 10000);
  assert.match(await caretLine(), /\b(Section|Paragraph) (9|10|11)\b/, "Show in source moves the source caret to the passage at the top of the preview (Section or Paragraph 10, within one)");
  // The source tints the line it landed on.
  await until("the source landing flash", () => exists(".cm-landing-flash"), 2000);

  // 4b. The same Show in source, pressed as a chord with the keyboard focus in
  // the preview: Ctrl+Alt+K reaches the Reader through the frame, not through
  // the window. The source caret starts at the end of the document, and the
  // preview is scrolled so Paragraph 15 is near its top.
  await scrollFrame((await paragraphTop(15)) - 40);
  await flush();
  await setCaret((await b.evaluate('window.EditorView.findFromDOM(document.querySelector(".cm-editor")).state.doc.length')));
  await flush();
  await b.evaluate(`(() => { const f = document.querySelector('iframe[title=Document]'); f.focus(); f.contentWindow.focus(); f.contentDocument.body.focus(); return true; })()`);
  await flush();
  // The chord must be heard inside the frame, not by the Reader's own window,
  // or this would only test the shortcut the Reader already had.
  await b.evaluate(frame(`w.__keys = []; d.addEventListener('keydown', e => w.__keys.push(e.key)); return true;`));
  assert.equal(await b.evaluate(`document.activeElement?.tagName`), "IFRAME", "the preview frame has the keyboard before the chord");
  // Browser key events go to the focused frame, and Mod is Ctrl on Linux: Ctrl (2) + Alt (1) = 3.
  await b.command("Input.dispatchKeyEvent", { type: "keyDown", modifiers: 3, key: "k", code: "KeyK", windowsVirtualKeyCode: 75 });
  await b.command("Input.dispatchKeyEvent", { type: "keyUp", modifiers: 3, key: "k", code: "KeyK", windowsVirtualKeyCode: 75 });
  assert.ok((await b.evaluate(frame(`return w.__keys;`))).includes("k"), "the chord was pressed inside the preview frame");
  await until("the source caret at the passage near the top of the preview from the preview's shortcut", async () => /\b(Section|Paragraph) (14|15|16)\b/.test(await caretLine()), 10000);
  assert.match(await caretLine(), /\b(Section|Paragraph) (14|15|16)\b/, "Ctrl+Alt+K pressed in the preview moves the source caret to the passage at the top of the preview (Section or Paragraph 15, within one)");

  // 5. A double-click on a passage in the preview moves the source caret there,
  // with Keep in step still off.
  await setCaret(0);
  await flush();
  await b.evaluate(frame(`const p = ${paragraphOf(20)}; p.scrollIntoView({ block: "center", behavior: "instant" }); return true;`));
  await flush();
  // A real double-click from the browser's pointer, at the start of the
  // paragraph's first line, in page coordinates.
  const point = await b.evaluate(frame(`const p = ${paragraphOf(20)}; const r = p.getClientRects()[0]; const box = f.getBoundingClientRect(); return { x: box.left + f.clientLeft + r.left + Math.min(40, r.width / 2), y: box.top + f.clientTop + r.top + r.height / 2 };`));
  assert.ok(point && point.x > 0 && point.y > 0, "Paragraph 20 has a place on screen to double-click: " + JSON.stringify(point));
  await b.command("Input.dispatchMouseEvent", { type: "mouseMoved", x: point.x, y: point.y });
  for (const clickCount of [1, 2]) {
    await b.command("Input.dispatchMouseEvent", { type: "mousePressed", x: point.x, y: point.y, button: "left", clickCount });
    await b.command("Input.dispatchMouseEvent", { type: "mouseReleased", x: point.x, y: point.y, button: "left", clickCount });
  }
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

  // 7. The divider pill sits between the two panes with the source on either
  // side. The default is the left; the right is set by the stored side and a reload.
  await assertPillBetweenPanes("left");
  await setSourceSide("right");
  await assertPillBetweenPanes("right");
  await setSourceSide("left");
  await assertPillBetweenPanes("left");

  console.log("show-in-panes-browser: Keep in step is on by default, its toggle sets the pill, Show in preview and Show in source move the panes with it off, Ctrl+Alt+K pressed in the preview moves the source, a double-click moves the source, the lock restores, and the pill sits between the panes on both sides");
} finally { await b?.close(); serverHttp?.close(); removeTemporary(temp); }
