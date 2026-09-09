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
const assets = { "asset.svg": new Uint8Array(readFileSync(join(CORPUS, "asset.svg"))) };

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
  intervals: compile("intervals.typ", readFileSync(join(REPO, "examples", "intervals.typ"), "utf8")),
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
window.seen = { ready: [], regions: [] };
addEventListener("message", (event) => {
  const message = event.data;
  if (!message || message.librepaper !== true) return;
  if (message.type === "ready") window.seen.ready.push(message.text);
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
      sameBounds: JSON.stringify(bounds()) === before };
    window.frames[0].postMessage({ librepaper: true, type: 'redlines', items: [] }, '*');
    await new Promise(resolve => setTimeout(resolve, 100));
    result.cleared = !doc.querySelector('mark.librepaper-ins, mark.librepaper-del');
    return result;
  `);
  check("PDF suggestions paint one proposal with a visible strike", tracked?.proposalCount === 1 && tracked.visibleStrike && tracked.proposalPosition === 'absolute', JSON.stringify(tracked));
  check("PDF redlines underline insertions without moving selectable text", tracked?.inserted === 'Typst PDF fixture' && tracked.underline.includes('underline') && tracked.sameText && tracked.sameBounds && tracked.deletionPosition === 'absolute', JSON.stringify(tracked));
  check("PDF redlines clear cleanly", tracked?.cleared, JSON.stringify(tracked));

  // Empty PDF end-of-line items must leave a separator in the live DOM.
  // Without it, this highlight becomes "parameteris fixed", fails to anchor,
  // and sorts after the comments instead of between them in the sidebar.
  await tab.eval("await window.sendPdf('/pdf/intervals')");
  await settle(tab);
  const positions = await tab.eval(`
    return [
      'A confidence interval is a statement about a procedure, not about a parameter.',
      'Sampling error is one source of uncertainty and rarely the largest.',
      'The parameter is fixed; the interval is what moved.',
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
