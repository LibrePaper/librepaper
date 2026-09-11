// Browser acceptance for the PDF viewer using the actual Typst compiler.
// This deliberately has no TeX dependency: the corpus is compiled by the
// same WASM ABI used by renderer-worker.js, then served to the real viewer.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { dirname, extname, join } from "node:path";
import { call, handOver } from "../src/lib/renderer-wasm.js";

const HERE = dirname(new URL(import.meta.url).pathname);
const REPO = dirname(dirname(HERE));
const SHELL = join(REPO, "web", "dist");
const CORPUS = join(HERE, "fixtures", "typst-corpus");
const WASM = join(SHELL, "wasm", "typst.wasm");

if (!existsSync(join(SHELL, "viewer.html")) || !existsSync(WASM)) {
  console.log("typst-viewer: no built viewer/WASM at web/dist; skipping (run `bun run build`)");
  process.exit(0);
}

const wasmModule = await WebAssembly.instantiate(readFileSync(WASM), {});
const wasm = wasmModule.instance.exports;
const corpusNames = ["paper.typ", "lib.typ", "long.typ", "refs.bib", "broken.typ"];
const texts = Object.fromEntries(corpusNames.map((name) => [name, readFileSync(join(CORPUS, name), "utf8")]));
const assets = {
  "asset.svg": new Uint8Array(readFileSync(join(CORPUS, "asset.svg"))),
  "librepaper-icon.png": new Uint8Array(readFileSync(join(REPO, "examples", "tutorial-typst", "librepaper-icon.png"))),
};

function compile(main, source = texts[main]) {
  handOver(wasm, { main, texts: { ...texts, [main]: source }, assets });
  const result = call(wasm, "compile", source, "Typst viewer fixture");
  assert.equal(result.kind, "pdf", `${main} did not produce a PDF`);
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
  return result.bytes;
}

const PDFs = {
  paper: compile("paper.typ"),
  long: compile("long.typ"),
  tutorial: compile("librepaper.typ", readFileSync(join(REPO, "examples", "tutorial-typst", "librepaper.typ"), "utf8")),
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const PORT = 8800 + Math.floor(Math.random() * 200);
const DEBUG_PORT = 9800 + Math.floor(Math.random() * 200);
const BASE = `http://localhost:${PORT}`;
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
};

// Keep the parent side intentionally close to Reader: it receives the agent's
// published text, asks the shared anchor implementation for offsets, and sends
// those offsets back to the PDF frame for painting.
const HARNESS = `<!doctype html><meta charset="utf-8"><body>
<iframe id="frame" src="/viewer.html" style="width:1000px;height:900px;border:0"></iframe>
<script type="module">
import { anchorOne, flatten } from "/src/lib/anchor.js";
const frame = document.getElementById("frame");
window.seen = { ready: [], regions: [], focus: [] };
addEventListener("message", (event) => {
  const message = event.data;
  if (!message || message.librepaper !== true) return;
  if (message.type === "ready") window.seen.ready.push(message.text);
  if (message.type === "focus") window.seen.focus.push(message.id);
  if (message.type === "regions-unplaceable") window.seen.regions.push(message);
});
window.sendPdf = async (path) => {
  const bytes = await fetch(path).then((r) => r.arrayBuffer());
  window.seen.ready.length = 0;
  frame.contentWindow.postMessage({ librepaper: true, type: "preview", pdf: bytes }, "*", [bytes]);
};
window.text = () => window.seen.ready.at(-1) || "";
window.anchor = (selector) => anchorOne(window.text(), selector, flatten(window.text()));
window.paint = (ranges) => frame.contentWindow.postMessage({ librepaper: true, type: "highlight", ranges }, "*");
window.paintRegions = (regions) => frame.contentWindow.postMessage({ librepaper: true, type: "regions", regions }, "*");
window.doc = () => frame.contentDocument;
window.ready = true;
</script></body>`;

const server = createServer((request, response) => {
  const path = new URL(request.url, BASE).pathname;
  if (path === "/" || path === "/harness.html") {
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(HARNESS);
    return;
  }
  if (path.startsWith("/pdf/") && PDFs[path.slice(5)]) {
    response.writeHead(200, { "content-type": "application/pdf" });
    response.end(PDFs[path.slice(5)]);
    return;
  }
  if (path.startsWith("/src/")) return sendFile(response, join(REPO, "web", path.slice(1)));
  const file = join(SHELL, path.replace(/^\/+/, ""));
  if (!file.startsWith(SHELL) || !existsSync(file)) {
    response.writeHead(404).end("not found");
    return;
  }
  if (path === "/viewer.html") {
    const page = readFileSync(file, "utf8").replace(
      "</body>",
      `<script src="/agent.js?reader=${BASE}"></script></body>`,
    );
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(page);
    return;
  }
  sendFile(response, file);
});

function sendFile(response, file) {
  if (!existsSync(file)) {
    response.writeHead(404).end("not found");
    return;
  }
  response.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" });
  response.end(readFileSync(file));
}

class Tab {
  constructor(socket, sessionId) {
    this.socket = socket;
    this.sessionId = sessionId;
    this.next = 1;
    this.pending = new Map();
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (!message.id || !this.pending.has(message.id)) return;
      const { resolve, reject } = this.pending.get(message.id);
      this.pending.delete(message.id);
      message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result);
    });
  }
  send(method, params = {}) {
    const id = this.next++;
    const payload = { id, method, params };
    if (this.sessionId) payload.sessionId = this.sessionId;
    this.socket.send(JSON.stringify(payload));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
  async eval(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression: `(async () => { ${expression} })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails).slice(0, 500));
    return result.result.value;
  }
}

const CHROME = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];
let chrome = null;
let socket = null;
async function endpoint() {
  for (let tries = 0; tries < 200; tries++) {
    try {
      return await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/version`).then((r) => r.json()).then((v) => v.webSocketDebuggerUrl);
    } catch {
      await wait(100);
    }
  }
  throw new Error("chromium never opened its debugging port");
}
async function settle(tab) {
  let last = -1;
  for (let tries = 0; tries < 120; tries++) {
    await wait(100);
    const now = await tab.eval("return window.text().length");
    if (now > 0 && now === last) return;
    last = now;
  }
  throw new Error("viewer did not publish PDF text");
}

let failures = 0;
const results = [];
function check(what, condition, detail = "") {
  results.push({ what, ok: Boolean(condition), detail });
  if (!condition) failures++;
}

async function run() {
  for (const candidate of CHROME) {
    chrome = spawn(candidate, [
      "--headless=new", `--remote-debugging-port=${DEBUG_PORT}`, "--no-sandbox",
      "--disable-gpu", "--disable-dev-shm-usage", "about:blank",
    ], { stdio: "ignore" });
    if (chrome) break;
  }
  if (!chrome) throw new Error("no Chromium to drive");
  socket = new WebSocket(await endpoint());
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve);
    socket.addEventListener("error", reject);
  });
  const root = new Tab(socket, null);
  const { targetId } = await root.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await root.send("Target.attachToTarget", { targetId, flatten: true });
  const tab = new Tab(socket, sessionId);
  await tab.send("Runtime.enable");
  await tab.send("Page.enable");
  await tab.send("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.workerStats = { created: 0, terminated: 0 };
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      constructor(...args) { super(...args); window.workerStats.created++; }
      terminate() { window.workerStats.terminated++; return super.terminate(); }
    };
  ` });
  await tab.send("Page.navigate", { url: `${BASE}/harness.html` });
  for (let tries = 0; tries < 200; tries++) {
    if (await tab.eval("return Boolean(window.ready && window.doc && window.doc())")) break;
    await wait(100);
  }

  await tab.eval("await window.sendPdf('/pdf/paper')");
  await settle(tab);
  const paper = await tab.eval(`
    const text = window.text();
    const pages = [...window.doc().querySelectorAll('.page')];
    return { text, pages: pages.length, marks: window.doc().querySelectorAll('.textLayer span:not(.gap)').length };
  `);
  check("Typst PDF paper is drawn with selectable text", paper.pages > 0 && paper.marks > 0, JSON.stringify(paper));
  const controls = await tab.eval(`
    const doc = frame.contentDocument;
    const win = frame.contentWindow;
    const toolbar = doc.querySelector('.pdf-toolbar').shadowRoot;
    const select = toolbar.getElementById('scaleSelect');
    const originalText = window.text();
    async function mode(value) {
      const old = doc.querySelector('.pages');
      select.value = value;
      select.dispatchEvent(new Event('change'));
      for (let i = 0; i < 100 && old === doc.querySelector('.pages'); i++)
        await new Promise(r => setTimeout(r, 50));
      if (old === doc.querySelector('.pages')) throw new Error('zoom did not render');
    }
    await mode('page-fit');
    const fit = doc.querySelector('.page').getBoundingClientRect();
    const fits = fit.width <= doc.documentElement.clientWidth && fit.height <= win.innerHeight - 40;
    await mode('2');
    const zoomWidth = doc.querySelector('.page').getBoundingClientRect().width;
    toolbar.getElementById('cursorHandTool').click();
    doc.querySelector('.page').dispatchEvent(new MouseEvent('mousedown', {bubbles:true, button:0, clientX:300, clientY:300}));
    doc.dispatchEvent(new MouseEvent('mousemove', {bubbles:true, buttons:1, clientX:200, clientY:200}));
    doc.dispatchEvent(new MouseEvent('mouseup', {bubbles:true}));
    const panned = doc.documentElement.scrollTop > 0;
    toolbar.getElementById('cursorSelectTool').click();
    const selection = !doc.documentElement.classList.contains('grab-to-pan-grab');
    await mode('page-width');
    const width = doc.querySelector('.page').getBoundingClientRect().width;
    const widthFits = Math.abs(width - (doc.documentElement.clientWidth - 32)) <= 1;
    await mode('auto');
    win.scrollTo(0,0);
    await new Promise(r => setTimeout(r, 500));
    return { fits, panned, selection, widthFits, zoomed: zoomWidth > width, sameText: originalText === window.text() };
  `);
  check("PDF controls fit, zoom, pan, and restore selection without changing text",
    Object.values(controls).every(Boolean), JSON.stringify(controls));
  const workers = await tab.eval("return frame.contentWindow.workerStats");
  check("PDF previews and zoom redraws reuse one worker", workers.created === 1 && workers.terminated === 0,
    JSON.stringify(workers));
  check("paper text keeps Unicode and the embedded figure caption", paper.text.includes("naïve café") && paper.text.includes("—") && paper.text.includes("An embedded SVG asset."), paper.text.slice(0, 500));
  check("Typst text extraction does not leak presentation ligature code points", paper.text.includes("fixture") && !/[\uFB00-\uFB06]/.test(paper.text), paper.text);
  const columns = await tab.eval(`
    const selector = { exact: 'As shown by', prefix: '', suffix: '' };
    const at = window.anchor(selector);
    if (!at) return null;
    window.paint([{ id: 'typst-column', start: at.start, end: at.end, motivation: 'commenting' }]);
    await new Promise((resolve) => setTimeout(resolve, 100));
    const page = window.doc().querySelector('.page');
    const spans = [...page.querySelectorAll('.textLayer span:not(.gap)')].filter((span) => span.textContent.trim());
    const lefts = spans.map((span) => span.getBoundingClientRect().left).sort((a, b) => a - b);
    const gap = lefts.slice(1).reduce((max, left, i) => Math.max(max, left - lefts[i]), 0);
    return { marks: page.querySelectorAll('mark[data-librepaper~="typst-column"]').length, gap };
  `);
  check("a text anchor paints in Typst's multi-column PDF layout", columns?.marks > 0 && columns.gap > 50, JSON.stringify(columns));
  const ligature = await tab.eval(`
    const at = window.anchor({ exact: 'fixture', prefix: 'Typst PDF ', suffix: '' });
    if (!at) return null;
    window.paint([{ id: 'typst-ligature', start: at.start, end: at.end, motivation: 'highlighting' }]);
    await new Promise((resolve) => setTimeout(resolve, 100));
    return { marks: window.doc().querySelectorAll('mark[data-librepaper~="typst-ligature"]').length, text: window.text().slice(at.start, at.end) };
  `);
  check("a ligature-safe Typst text anchor paints", ligature?.marks > 0 && ligature.text === "fixture", JSON.stringify(ligature));

  const tracked = await tab.eval(`
    const doc = window.doc();
    const at = window.anchor({ exact: 'Typst PDF fixture', prefix: '', suffix: '' });
    if (!at) return null;
    const spans = [...doc.querySelectorAll('.textLayer span:not(.gap)')];
    const bounds = () => spans.map(span => {
      const r = doc.createRange(); r.selectNodeContents(span);
      const b = r.getBoundingClientRect();
      return [b.x, b.y, b.width, b.height];
    });
    const before = JSON.stringify(bounds());
    const text = doc.body.textContent;
    window.paint([{ id: 'tracked', start: at.start, end: at.end, motivation: 'editing', proposed: 'A revised title' }]);
    await new Promise(resolve => setTimeout(resolve, 100));
    const proposals = doc.querySelectorAll('mark[data-proposed]');
    const strike = doc.querySelector('mark[data-librepaper~="tracked"]');
    const style = doc.defaultView.getComputedStyle(strike);
    const proposalCount = proposals.length;
    const proposalPosition = doc.defaultView.getComputedStyle(proposals[0], '::after').position;
    window.paint([]);
    window.frames[0].postMessage({ librepaper: true, type: 'redlines', items: [
      { kind: 'insert', start: at.start, end: at.end, who: 'Editor' },
      { kind: 'delete', at: at.start, text: 'Previous title', who: 'Editor' },
    ] }, '*');
    await new Promise(resolve => setTimeout(resolve, 100));
    const insertion = doc.querySelector('mark.librepaper-ins');
    const deletion = doc.querySelector('mark.librepaper-del');
    const result = { proposalCount, proposalPosition,
      visibleStrike: style.textDecorationColor !== 'rgba(0, 0, 0, 0)',
      inserted: [...doc.querySelectorAll('mark.librepaper-ins')].map(m => m.textContent).join(''),
      deletionPosition: doc.defaultView.getComputedStyle(deletion, '::before').position,
      underline: doc.defaultView.getComputedStyle(insertion).textDecorationLine,
      sameText: doc.body.textContent === text,
      // Splitting text nodes can round glyph widths by 1/64 CSS pixel at fractional zoom.
      sameBounds: bounds().every((box, i) => box.every((value, j) => Math.abs(value - JSON.parse(before)[i][j]) < 0.05)) };
    window.frames[0].postMessage({ librepaper: true, type: 'redlines', items: [] }, '*');
    await new Promise(resolve => setTimeout(resolve, 100));
    result.cleared = !doc.querySelector('mark.librepaper-ins, mark.librepaper-del');
    return result;
  `);
  check("PDF suggestions paint one proposal with a visible strike", tracked?.proposalCount === 1 && tracked.visibleStrike && tracked.proposalPosition === 'absolute', JSON.stringify(tracked));
  check("PDF redlines underline insertions without moving selectable text", tracked?.inserted === 'Typst PDF fixture' && tracked.underline.includes('underline') && tracked.sameText && tracked.sameBounds && tracked.deletionPosition === 'absolute', JSON.stringify(tracked));
  check("PDF redlines clear cleanly", tracked?.cleared, JSON.stringify(tracked));

  // Point comments use an empty text quote and a zero-width inline marker. A
  // custom colour must remain visible in the PDF text layer, while the marker
  // must not become part of the published text or trigger a ready loop.
  const point = await tab.eval(`
    const doc = window.doc();
    const text = window.text();
    const at = window.anchor({ exact: 'Typst PDF fixture', prefix: '', suffix: '' });
    if (!at) return null;
    const pointAt = at.start + 5;
    const selector = {
      exact: '', prefix: text.slice(Math.max(0, pointAt - 12), pointAt),
      suffix: text.slice(pointAt, pointAt + 12), position: pointAt, point: true,
    };
    const anchored = window.anchor(selector);
    if (!anchored) return { error: 'point failed to anchor' };
    const readyBefore = window.seen.ready.length;
    window.seen.focus.length = 0;
    window.paint([
      { id: 'pdf-point', ...anchored, point: true, motivation: 'commenting' },
      { id: 'pdf-colour', start: at.start, end: at.end, motivation: 'highlighting', color: '#ff8800' },
    ]);
    await new Promise(resolve => setTimeout(resolve, 150));
    const marker = doc.querySelector('.librepaper-point-marker');
    const bubble = doc.querySelector('.librepaper-point-bubble');
    const mark = doc.querySelector('mark[data-librepaper~="pdf-colour"]');
    const first = marker?.getBoundingClientRect();
    const before = { text, readyBefore, markers: doc.querySelectorAll('.librepaper-point-marker').length,
      bubbleText: bubble?.textContent || '', markColour: doc.defaultView.getComputedStyle(mark).backgroundColor,
      marker: first ? [first.left, first.top, first.width, first.height] : null };
    bubble?.click();
    await new Promise(resolve => setTimeout(resolve, 80));
    before.focused = window.seen.focus.includes('pdf-point');
    before.textStable = window.text() === text && window.seen.ready.length === readyBefore;
    return before;
  `);
  check("PDF point comment has a textless anchored marker and custom colour", point?.markers === 1 && point.bubbleText === '' && point.textStable && point.markColour && !/transparent|rgba\(0, 0, 0, 0\)/.test(point.markColour), JSON.stringify(point));
  check("PDF point bubble focuses its comment thread", point?.focused, JSON.stringify(point));

  const pointZoom = await tab.eval(`
    const doc = window.doc();
    const toolbar = doc.querySelector('.pdf-toolbar').shadowRoot;
    const select = toolbar.getElementById('scaleSelect');
    const marker = () => doc.querySelector('.librepaper-point-marker')?.getBoundingClientRect();
    const before = marker();
    select.value = '2'; select.dispatchEvent(new Event('change'));
    for (let i = 0; i < 100 && !marker(); i++) await new Promise(r => setTimeout(r, 50));
    await new Promise(r => setTimeout(r, 150));
    const zoomed = marker();
    const text = window.text();
    // Reanchor using the same contextual selector as the parent would after
    // rerender; then repaint the actual point at its original location.
    const pointAt = text.indexOf('Typst PDF fixture') + 5;
    const anchored = window.anchor({ exact: '', prefix: text.slice(pointAt - 12, pointAt), suffix: text.slice(pointAt, pointAt + 12), position: pointAt, point: true });
    window.paint([{ id: 'pdf-point', ...anchored, point: true, motivation: 'commenting' }, { id: 'pdf-colour', start: pointAt - 5, end: pointAt + 12, motivation: 'highlighting', color: '#ff8800' }]);
    await new Promise(r => setTimeout(r, 120));
    const rerendered = marker();
    return { before: before && [before.left, before.top], zoomed: zoomed && [zoomed.left, zoomed.top], rerendered: rerendered && [rerendered.left, rerendered.top], text, markers: doc.querySelectorAll('.librepaper-point-marker').length };
  `);
  check("PDF point marker follows zoom and parent repaint", pointZoom?.markers === 1 && pointZoom?.text.includes('Typst PDF fixture') && pointZoom.rerendered?.every(Number.isFinite), JSON.stringify(pointZoom));

  // Empty PDF end-of-line items must leave a separator in the live DOM.
  // Without it, this highlight becomes "parameteris fixed", fails to anchor,
  // and sorts after the comments instead of between them in the sidebar.
  await tab.eval("window.paint([]); await new Promise(r => setTimeout(r, 80)); await window.sendPdf('/pdf/tutorial')");
  await settle(tab);
  const positions = await tab.eval(`
    return [
      'Typst combines markup, math, and scripting in one compact source file.',
      'A small function can keep repeated labels consistent:',
      'Try changing the estimate and leave a comment on this paragraph.',
    ].map((exact) => window.anchor({ exact })?.start ?? null);
  `);
  check("a highlight across a Typst line break anchors between comments",
    positions.every(Number.isFinite) && positions[0] < positions[2] && positions[2] < positions[1],
    JSON.stringify(positions));

  await tab.eval("await window.sendPdf('/pdf/long')");
  await settle(tab);
  const long = await tab.eval(`
    const text = window.text();
    const pages = [...window.doc().querySelectorAll('.page')];
    return { text, pages: pages.length, pageText: pages.map((p) => p.querySelector('.textLayer')?.textContent || '') };
  `);
  check("long Typst PDF has actual page breaks", long.pages > 1 && long.text.includes("Section 23"), JSON.stringify({ pages: long.pages, chars: long.text.length }));
  check("long PDF text keeps Unicode across repeated sections", long.text.includes("Montréal") && long.text.includes("β") && long.text.includes("selectable"), long.text.slice(0, 500));

  // A quote spanning the first PDF page boundary exercises the same offsets
  // and pageForOffset path used by Reader. The source is selected from the
  // actual extracted text, so this remains useful if Typst reflows the corpus.
  const boundary = await tab.eval(`
    const text = window.text();
    const pages = [...window.doc().querySelectorAll('.page')];
    let cursor = 0, hit = null;
    for (let i = 0; i + 1 < pages.length; i++) {
      const one = pages[i].querySelector('.textLayer')?.textContent || '';
      const at = text.indexOf(one, cursor);
      if (at < 0) continue;
      const next = pages[i + 1].querySelector('.textLayer')?.textContent || '';
      const nextAt = text.indexOf(next, at + one.length);
      if (nextAt > at && one.trim() && next.trim()) {
        const start = Math.max(at + one.length - 28, at);
        const end = Math.min(nextAt + 28, text.length);
        hit = { selector: { exact: text.slice(start, end), prefix: '', suffix: '' }, boundary: [start, end], pages: pages.length };
        break;
      }
      cursor = at + one.length;
    }
    return hit;
  `);
  check("a text anchor is found across a real Typst page break", Boolean(boundary?.selector), JSON.stringify(boundary));
  if (boundary?.selector) {
    const found = await tab.eval(`
      const at = window.anchor(${JSON.stringify(boundary.selector)});
      if (!at) return null;
      window.paint([{ id: 'typst-cross-page', start: at.start, end: at.end, motivation: 'editing', proposed: 'A replacement across pages' }]);
      await new Promise((resolve) => setTimeout(resolve, 120));
      const viewer = frame.contentWindow.librepaperViewer;
      const marks = [...window.doc().querySelectorAll('mark[data-librepaper]')].filter((m) => !m.closest('span.gap'));
      const markPages = [...new Set(marks.map((mark) => mark.closest('.page')?.dataset.page).filter(Boolean))];
      return { at, startPage: viewer.pageForOffset(at.start), endPage: viewer.pageForOffset(at.end - 1), markPages, marks: marks.length, proposals: window.doc().querySelectorAll('mark[data-proposed]').length };
    `);
    check("the cross-page text anchor paints in the PDF text layer", found?.marks > 0, JSON.stringify(found));
    check("the cross-page highlight reaches both pages", found?.markPages?.length > 1, JSON.stringify(found));
    check("a suggestion spanning PDF pages shows its proposal only once", found?.proposals === 1, JSON.stringify(found));
    check("PDF pageForOffset follows the highlighted page break", found?.endPage > found?.startPage, JSON.stringify(found));
  }

  const known = await tab.eval(`
    const selector = { exact: 'Montréal, β, and', prefix: 'contains Unicode: ', suffix: '' };
    const at = window.anchor(selector);
    if (!at) return { index: window.text().indexOf('Montréal, β, and'), sample: window.text().slice(window.text().indexOf('Montréal') - 20, window.text().indexOf('Montréal') + 80) };
    window.paint([{ id: 'typst-unicode', start: at.start, end: at.end, motivation: 'highlighting' }]);
    await new Promise((resolve) => setTimeout(resolve, 100));
    return { at, marks: window.doc().querySelectorAll('mark[data-librepaper~="typst-unicode"]').length };
  `);
  check("a repeated Unicode anchor is painted through the shared anchor code", known?.marks > 0, JSON.stringify(known));

  await tab.eval(`
    window.seen.regions.length = 0;
    window.paintRegions([{ id: 'legacy-figure', digest: 'old-html-image', index: 0, x: 10, y: 10, w: 20, h: 20 }]);
    await new Promise((resolve) => setTimeout(resolve, 120));
  `);
  const region = await tab.eval("return window.seen.regions.at(-1) || null");
  check("legacy HTML figure regions are explicitly unplaceable in a Typst PDF", region?.reason === "pdf" && region.ids.includes("legacy-figure"), JSON.stringify(region));

  // The history panel's prev/next steps send `locate` at the offset of a
  // change, the same offset `redlines` painted at. The frame is still showing
  // the multi-page "long" PDF from the pageForOffset check above, so this
  // exercises a scroll to a passage well past the first page.
  const located = await tab.eval(`
    const win = frame.contentWindow;
    const text = window.text();
    const at = text.indexOf('Section 3');
    win.scrollTo(0, 0);
    await new Promise((resolve) => setTimeout(resolve, 50));
    const before = win.scrollY;
    win.postMessage({ librepaper: true, type: 'locate', start: at, length: 'Section 3'.length }, '*');
    await new Promise((resolve) => setTimeout(resolve, 500));
    const after = win.scrollY;
    return { at, before, after };
  `);
  check("locate scrolls the PDF viewer to a passage offset", located?.at > 0 && located.after > located.before, JSON.stringify(located));

  await root.send("Target.closeTarget", { targetId });
}

server.listen(PORT);
try {
  await run();
} catch (error) {
  check("the Typst viewer check ran", false, String(error).slice(0, 500));
} finally {
  try { socket?.close(); } catch {}
  chrome?.kill();
  server.close();
  await wait(150);
}

for (const { what, ok, detail } of results) console.log(`typst-viewer: ${ok ? "ok  " : "FAIL"}  ${what}${detail ? ` -- ${detail}` : ""}`);
if (failures) {
  console.error(`typst-viewer: ${failures} of ${results.length} checks failed`);
  process.exit(1);
}
console.log(`typst-viewer: ${results.length} browser checks passed`);
