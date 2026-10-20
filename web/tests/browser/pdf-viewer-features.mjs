// Browser acceptance for the features of the PDF frame: links, sidebar,
// outline, search, page jumps, spreads, density, invert, presentation and
// ctrl+wheel zoom.
//
// A six page Typst document is compiled with the pinned typst.wasm (the same
// ABI renderer-worker.js uses), served to the real web/dist viewer.html with
// frame.js injected, and driven the way the reader drives it: the harness page
// stands in for the host, posts the same `viewer-*` messages and records the
// `viewer-state` and `ready` messages the frame sends back. Pointer input
// (clicks and wheel events) goes through CDP Input, so it takes the same path
// a real mouse does. The document is built to need each feature: headings for
// the outline, internal links several pages ahead, an external link, and a
// word that occurs exactly five times between pages 2 and 5 for the search.
//
// Without web/dist or the pinned Typst wasm this prints a message and exits 0.

import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { dirname, extname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { call, handOver } from "../../src/lib/renderer-wasm.js";
import { browser, removeTemporary } from "../helpers/browser-driver.mjs";
import { assertFreshDist } from "../helpers/fresh-dist.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = dirname(dirname(dirname(HERE)));
const SHELL = join(REPO, "web", "dist");
const WASM = join(REPO, "web", "wasm", "typst.wasm");

if (!existsSync(join(SHELL, "viewer.html"))) {
  console.log("pdf-viewer-features: no built viewer at web/dist; skipping (run `bun run build` from web/)");
  process.exit(0);
}
assertFreshDist("pdf-viewer-features");
if (!existsSync(WASM)) {
  console.log("pdf-viewer-features: no pinned Typst WASM at web/wasm; skipping (run `node tools/assets/pins.mjs fetch` from the repository root)");
  process.exit(0);
}

const HEADINGS = ["Introduction", "Method", "Results", "Discussion", "Appendix", "Conclusion"];
// Page 1 links forward to the results (page 3) and the conclusion (page 6),
// and out to example.org. The search word sits twice on page 2 and once on
// each of pages 3, 4 and 5.
const SOURCE = `#set heading(numbering: "1.")
#set page(margin: 40pt)
#set par(justify: false)

= Introduction
This document exercises the viewer. Jump ahead with #link(<results>)[the results section],
read @conclusion, or visit #link("https://example.org")[example] for more.

#pagebreak()
= Method
The first animal swims in a tank. The zebrafish is small and quick.
Another sentence follows about a zebrafish in the same tank.

#pagebreak()
= Results <results>
One zebrafish was counted here and nothing else of note.

#pagebreak()
= Discussion
The last zebrafish of the day was counted on this page.

#pagebreak()
= Appendix
A final zebrafish appears in the appendix, with a #link(<conclusion>)[link to the end].

#pagebreak()
= Conclusion <conclusion>
Nothing more to say.
`;

const wasmModule = await WebAssembly.instantiate(readFileSync(WASM), {});
const wasm = wasmModule.instance.exports;
handOver(wasm, { main: "features.typ", texts: { "features.typ": SOURCE }, assets: {} });
const compiled = call(wasm, "compile", SOURCE, "PDF viewer features fixture");
assert.equal(compiled.kind, "pdf", "the features document did not produce a PDF");
assert.equal(compiled.ok, true, JSON.stringify(compiled.diagnostics));
const PDFs = { features: compiled.bytes };

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
// The page server takes whatever port the system hands it, and the DevTools
// port sits in a range no other browser test uses, so this file can run
// beside typst-viewer.mjs and latex-browser.mjs under `--test-concurrency`.
const DEBUG_PORT = 24000 + Math.floor(Math.random() * 1000);
let chrome = null;
let profile = null;
let BASE = "http://localhost";
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
};

// The parent side stands in for the host. It keeps every `ready` text the
// frame publishes since the last preview and the latest `viewer-state`.
const HARNESS = `<!doctype html><meta charset="utf-8"><body style="margin:0">
<iframe id="frame" src="/viewer.html" style="display:block;width:1000px;height:900px;border:0"></iframe>
<script type="module">
const frame = document.getElementById("frame");
window.seen = { ready: [], state: null };
addEventListener("message", (event) => {
  const message = event.data;
  if (!message || message.librepaper !== true) return;
  if (message.type === "ready") window.seen.ready.push(message.text);
  if (message.type === "viewer-state") window.seen.state = message;
});
window.sendPdf = async (path) => {
  const bytes = await fetch(path).then((r) => r.arrayBuffer());
  window.seen.ready.length = 0;
  frame.contentWindow.postMessage({ librepaper: true, type: "preview", pdf: bytes }, "*", [bytes]);
};
window.ask = (message) => frame.contentWindow.postMessage({ librepaper: true, ...message }, "*");
window.text = () => window.seen.ready.at(-1) || "";
window.state = () => window.seen.state;
window.doc = () => frame.contentDocument;
window.win = () => frame.contentWindow;
window.page = (n) => window.doc().querySelector('.page[data-page="' + n + '"]');
window.side = () => {
  const host = [...window.doc().body.children].find((node) => node.shadowRoot && node.shadowRoot.querySelector(".panel"));
  return host ? { host, root: host.shadowRoot } : null;
};
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
  const file = join(SHELL, path.replace(/^\/+/, ""));
  if (!file.startsWith(SHELL) || !existsSync(file)) {
    response.writeHead(404).end("not found");
    return;
  }
  if (path === "/viewer.html") {
    const page = readFileSync(file, "utf8").replace(
      "</body>",
      `<script src="/frame.js?reader=${BASE}"></script></body>`,
    );
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(page);
    return;
  }
  response.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" });
  response.end(readFileSync(file));
});

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

// Poll an expression (evaluated in the harness page, `return`ed as a boolean)
// until it holds or the time runs out.
async function until(tab, expression, ms = 6000) {
  const end = Date.now() + ms;
  for (;;) {
    if (await tab.eval(`return Boolean(${expression})`)) return true;
    if (Date.now() > end) return false;
    await wait(100);
  }
}

let failures = 0;
const results = [];
function check(what, condition, detail = "") {
  results.push({ what, ok: Boolean(condition), detail });
  if (!condition) failures++;
}

async function run() {
  profile = mkdtempSync(join(tmpdir(), "librepaper-pdf-viewer-features-"));
  chrome = await browser("chromium", profile, DEBUG_PORT);
  const tab = {
    send: (method, params = {}) => chrome.command(method, params),
    eval: (expression) => chrome.evaluate(`(async () => { ${expression} })()`),
  };
  await tab.send("Runtime.enable");
  await tab.send("Page.enable");
  // The iframe is 1000 by 900 at the top left of the page, so a point in the
  // frame is the same point in the browser's viewport.
  await tab.send("Emulation.setDeviceMetricsOverride", { width: 1000, height: 900, deviceScaleFactor: 1, mobile: false });
  await tab.send("Page.navigate", { url: `${BASE}/harness.html` });
  // The frame says "nothing drawn" once its listener is up; a preview posted
  // before that lands in the blank document the iframe starts with.
  for (let tries = 0; tries < 200; tries++) {
    if (await tab.eval("return Boolean(window.ready && window.state())")) break;
    await wait(100);
  }

  // A real click or wheel event at a point of the frame.
  const mouse = (type, x, y, extra = {}) => tab.send("Input.dispatchMouseEvent", { type, x, y, ...extra });
  async function click(x, y) {
    await mouse("mouseMoved", x, y);
    await mouse("mousePressed", x, y, { button: "left", clickCount: 1 });
    await mouse("mouseReleased", x, y, { button: "left", clickCount: 1 });
  }
  // Scroll an element (an expression evaluated in the harness) to the middle
  // of the viewport and return the centre of its box in viewport pixels.
  async function centre(element) {
    return tab.eval(`
      const target = (${element});
      if (!target) return null;
      target.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
      await new Promise((resolve) => setTimeout(resolve, 250));
      const box = target.getBoundingClientRect();
      const origin = document.getElementById("frame").getBoundingClientRect();
      return { x: origin.left + box.left + box.width / 2, y: origin.top + box.top + box.height / 2,
        page: Number(target.dataset.page) || null, href: target.getAttribute("href") };
    `);
  }
  const toTop = async () => {
    await tab.eval("window.win().scrollTo({ top: 0, left: 0, behavior: 'instant' }); await new Promise((r) => setTimeout(r, 400));");
    await until(tab, "window.state().page === 1");
  };

  await tab.eval("await window.sendPdf('/pdf/features')");
  await settle(tab);
  await until(tab, "window.state() && window.state().drawn && window.state().pages > 0");
  await wait(500);

  // 1. The state the frame reports once the PDF is drawn.
  const first = await tab.eval(`
    const state = window.state();
    return { state, canvases: window.doc().querySelectorAll('.page canvas').length };
  `);
  check("viewer-state carries the page count and the default switches",
    first.state?.drawn === true && first.state.page >= 1 && first.state.pages === first.canvases && first.canvases === 6
      && first.state.sidebar === false && first.state.spread === false && first.state.invert === false
      && first.state.presenting === false && first.state.find === null,
    JSON.stringify(first));

  // 2. Links.
  const layers = await tab.eval(`
    const pages = [...window.doc().querySelectorAll('.page')];
    return { pages: pages.length, layers: pages.filter((p) => p.querySelector('.linkLayer')).length,
      internal: window.doc().querySelectorAll('a[href^="#page="]').length,
      external: window.doc().querySelectorAll('a[href="https://example.org/"], a[href="https://example.org"]').length };
  `);
  check("every page has a link layer with internal and external links",
    layers.pages === layers.layers && layers.internal > 0 && layers.external > 0, JSON.stringify(layers));

  await toTop();
  const internal = await centre(`[...window.doc().querySelectorAll('a[href^="#page="]')]
    .find((a) => Number(a.dataset.page) > Number(a.closest('.page').dataset.page) + 1)`);
  let jumped = null;
  if (internal) {
    await click(internal.x, internal.y);
    jumped = await until(tab, `window.state().page === ${internal.page}`, 3000);
  }
  check("clicking an internal link goes to its target page", Boolean(internal) && jumped,
    JSON.stringify({ internal, state: await tab.eval("return window.state()") }));

  await toTop();
  const external = await centre(`window.doc().querySelector('a[href="https://example.org/"], a[href="https://example.org"]')`);
  let opened = [];
  if (external) {
    await tab.eval(`
      const win = window.win();
      win.__opened = [];
      win.open = (...args) => { win.__opened.push(args); return null; };
    `);
    await click(external.x, external.y);
    await wait(300);
    opened = await tab.eval("return window.win().__opened");
  }
  check("clicking an external link calls window.open with its address",
    opened.length === 1 && /^https:\/\/example\.org\/?$/.test(opened[0][0]), JSON.stringify({ external, opened }));

  // 3. Links and the sidebar add no text. The agent's published text is the
  // same before and after the sidebar opens, and every republish in between.
  await toTop();
  const before = await tab.eval("return { text: window.text(), count: window.seen.ready.length }");

  // 4. The sidebar.
  await tab.eval("window.ask({ type: 'viewer-sidebar', open: true })");
  const opening = await until(tab, "window.state().sidebar === true && window.side() && window.side().host.hasAttribute('open')");
  const thumbs = await until(tab, "window.side() && window.side().root.querySelector('.thumb canvas')", 8000);
  const outlined = await until(tab, `(() => {
    const side = window.side();
    const tabs = side && side.root.querySelectorAll('.tabs button');
    return tabs && tabs[1] && !tabs[1].hidden && side.root.querySelectorAll('.outline .link').length >= ${HEADINGS.length};
  })()`, 8000);
  await wait(1500);
  const after = await tab.eval("return { texts: window.seen.ready.slice(" + before.count + "), last: window.text() }");
  check("opening the sidebar changes no published text and adds no Pages or Outline words",
    before.text.length > 0 && after.last === before.text && after.texts.every((text) => text === before.text)
      && !before.text.includes("Pages") && !before.text.includes("Outline") && !after.last.includes("Pages") && !after.last.includes("Outline"),
    JSON.stringify({ before: before.text.slice(0, 200), after: after.last.slice(0, 200) }));

  const sidebar = await tab.eval(`
    const side = window.side();
    const tabs = [...side.root.querySelectorAll('.tabs button')];
    const outline = tabs[1];
    const box = outline.getBoundingClientRect();
    const links = [...side.root.querySelectorAll('.outline .link')].map((node) => node.textContent);
    return { open: side.host.hasAttribute('open'), state: window.state().sidebar,
      thumbnails: side.root.querySelectorAll('.thumb canvas').length,
      outlineVisible: !outline.hidden && box.width > 0 && box.height > 0, links,
      margin: getComputedStyle(window.doc().querySelector('main')).marginLeft };
  `);
  check("the sidebar opens in a shadow root and reports it", opening && sidebar.open && sidebar.state === true, JSON.stringify(sidebar));
  check("the sidebar draws page thumbnails", thumbs && sidebar.thumbnails >= 1, JSON.stringify(sidebar));
  check("the Outline tab lists the headings",
    outlined && sidebar.outlineVisible && HEADINGS.every((title) => sidebar.links.some((link) => link.includes(title))),
    JSON.stringify(sidebar));
  check("the page column makes room for the sidebar", sidebar.margin === "200px", sidebar.margin);

  // 5. A recompile keeps the tab the reader chose.
  await tab.eval("window.side().root.querySelectorAll('.tabs button')[1].click()");
  const chosen = await tab.eval("return window.side().root.querySelectorAll('.tabs button')[1].getAttribute('aria-selected')");
  await tab.eval("await window.sendPdf('/pdf/features')");
  await settle(tab);
  await wait(2000);
  const kept = await tab.eval(`
    const tabs = window.side().root.querySelectorAll('.tabs button');
    const outline = window.side().root.querySelector('.body.outline');
    return { selected: tabs[1].getAttribute('aria-selected'), hidden: tabs[1].hidden, panelHidden: outline.hidden };
  `);
  check("a recompile keeps the Outline tab selected",
    chosen === "true" && kept.selected === "true" && !kept.hidden && !kept.panelHidden, JSON.stringify({ chosen, kept }));

  // 6. Search.
  await toTop();
  await tab.eval("window.ask({ type: 'viewer-find', query: 'zebrafish', step: 0 })");
  const found = await until(tab, "window.state().find && window.state().find.total > 0");
  const findFirst = await tab.eval("return window.state().find");
  await tab.eval("window.ask({ type: 'viewer-find', query: 'zebrafish', step: 1 })");
  const stepped = await until(tab, "window.state().find && window.state().find.current === 2");
  const highlighted = await tab.eval(`
    const highlights = window.win().CSS.highlights;
    return { all: highlights.get('lp-find')?.size ?? 0, current: highlights.get('lp-find-current')?.size ?? 0 };
  `);
  await tab.eval("window.ask({ type: 'viewer-find', query: '', step: 0 })");
  const cleared = await until(tab, "window.state().find === null");
  const gone = await tab.eval("return { all: window.win().CSS.highlights.has('lp-find'), current: window.win().CSS.highlights.has('lp-find-current') }");
  check("searching reports the first of five matches", found && findFirst.current === 1 && findFirst.total === 5, JSON.stringify(findFirst));
  check("stepping moves to the second match", stepped, JSON.stringify(await tab.eval("return window.state().find")));
  check("all five matches are highlighted without touching the DOM", highlighted.all === 5 && highlighted.current === 1, JSON.stringify(highlighted));
  check("an empty query clears the search and its highlights", cleared && !gone.all && !gone.current, JSON.stringify(gone));

  // 6b. Painting a comment over a match must not collapse the search highlights.
  await tab.eval("window.ask({ type: 'viewer-find', query: 'zebrafish', step: 0 })");
  await until(tab, "window.state().find && window.state().find.total === 5");
  await tab.eval(`
    const at = window.text().indexOf('zebrafish');
    window.win().postMessage({ librepaper: true, type: 'highlight',
      ranges: [{ id: 'over-find', start: at + 3, end: at + 20, motivation: 'highlighting' }] }, '*');
  `);
  await until(tab, "window.doc().querySelector('mark[data-librepaper]')");
  await wait(500);
  const survived = await tab.eval(`
    const set = window.win().CSS.highlights.get('lp-find');
    const ranges = set ? [...set] : [];
    return { size: ranges.length, collapsed: ranges.filter((range) => range.collapsed).length,
      texts: ranges.map((range) => range.toString().toLowerCase()) };
  `);
  check("painting a comment over a match leaves all five search highlights intact",
    survived.size === 5 && survived.collapsed === 0 && survived.texts.every((text) => text === "zebrafish"), JSON.stringify(survived));
  await tab.eval("window.win().postMessage({ librepaper: true, type: 'highlight', ranges: [] }, '*')");
  await tab.eval("window.ask({ type: 'viewer-find', query: '', step: 0 })");
  await until(tab, "window.state().find === null");

  // 6c. A diagonal wheel over the sidebar scrolls its list, not the window.
  await tab.eval("window.ask({ type: 'viewer-scale', mode: '3' })");
  await until(tab, "window.state().scale && window.state().mode === '3'");
  await tab.eval("window.ask({ type: 'viewer-sidebar', open: true })");
  await until(tab, "window.state().sidebar === true && window.side() && window.side().host.hasAttribute('open')");
  // Check 5 left the Outline tab selected, and the outline is too short to scroll.
  await tab.eval("window.side().root.querySelectorAll('.tabs button')[0].click()");
  await wait(1500);
  const lockBefore = await tab.eval(`
    const root = window.doc().documentElement;
    const list = window.side().root.querySelector('.body.pages');
    return { overflows: root.scrollWidth > root.clientWidth + 1, scrollable: list.scrollHeight > list.clientHeight,
      scrollY: window.win().scrollY, top: list.scrollTop };
  `);
  await mouse("mouseWheel", 100, 400, { deltaX: 20, deltaY: 200 });
  await wait(500);
  const lockAfter = await tab.eval(`
    const list = window.side().root.querySelector('.body.pages');
    return { scrollY: window.win().scrollY, top: list.scrollTop };
  `);
  check("a diagonal wheel over the sidebar scrolls its list and leaves the window alone",
    lockBefore.overflows && lockBefore.scrollable && lockAfter.top > lockBefore.top && lockAfter.scrollY === lockBefore.scrollY,
    JSON.stringify({ lockBefore, lockAfter }));
  await tab.eval("window.ask({ type: 'viewer-scale', mode: 'auto' })");
  await until(tab, "window.state().mode === 'auto'");
  await settle(tab);
  await wait(1000);
  await toTop();

  // 7. Page jump.
  await tab.eval("window.ask({ type: 'viewer-page', page: 4 })");
  const landed = await until(tab, "window.state().page === 4 && Math.abs(window.page(4).getBoundingClientRect().top) <= 5", 4000);
  check("viewer-page puts the page at the top of the viewport", landed,
    JSON.stringify(await tab.eval("return { state: window.state().page, top: window.page(4).getBoundingClientRect().top }")));

  // 8. A recompile keeps the reading position.
  const middle = await tab.eval(`
    const win = window.win();
    const rect = window.page(3).getBoundingClientRect();
    win.scrollTo({ top: win.scrollY + rect.top + rect.height * 0.3, behavior: 'instant' });
    await new Promise((resolve) => setTimeout(resolve, 400));
    return { page: window.state().page, scrollY: win.scrollY };
  `);
  await until(tab, "window.state().page === 3");
  await tab.eval("await window.sendPdf('/pdf/features')");
  await settle(tab);
  await wait(1500);
  const kept3 = await tab.eval("return { page: window.state().page, scrollY: window.win().scrollY }");
  check("a recompile keeps the page and the scroll position",
    kept3.page === 3 && Math.abs(kept3.scrollY - middle.scrollY) <= 30, JSON.stringify({ middle, kept3 }));

  // 9. A page that fits has nothing to scroll sideways.
  await tab.eval("window.ask({ type: 'viewer-sidebar', open: false })");
  await until(tab, "window.state().sidebar === false && !window.side().host.hasAttribute('open')");
  await wait(1500);
  const fits = await tab.eval(`
    const root = window.doc().documentElement;
    return { scrollWidth: root.scrollWidth, clientWidth: root.clientWidth };
  `);
  await mouse("mouseWheel", 500, 400, { deltaX: 200, deltaY: 0 });
  await wait(400);
  const sideways = await tab.eval("return window.win().scrollX");
  check("a page that fits does not scroll sideways, by layout or by wheel",
    fits.scrollWidth <= fits.clientWidth && sideways === 0, JSON.stringify({ fits, sideways }));

  // 10. Spread: 1 alone on the right, then 2-3, 4-5.
  await tab.eval("window.ask({ type: 'viewer-spread', spread: true })");
  const spreadOn = await until(tab, "window.state().spread === true && window.doc().querySelector('.pages.spread')");
  const rows = await tab.eval(`
    const left = (n) => window.page(n).getBoundingClientRect().left;
    const top = (n) => window.page(n).getBoundingClientRect().top;
    return { top2: top(2), top3: top(3), left1: left(1), left2: left(2), left3: left(3) };
  `);
  check("a spread puts pages 2 and 3 side by side and page 1 on the right",
    spreadOn && Math.abs(rows.top2 - rows.top3) <= 1 && rows.left1 > rows.left2 && rows.left3 > rows.left2, JSON.stringify(rows));
  await tab.eval("window.ask({ type: 'viewer-spread', spread: false })");
  const spreadOff = await until(tab, "window.state().spread === false && !window.doc().querySelector('.pages.spread')");
  const column = await tab.eval(`
    const left = (n) => window.page(n).getBoundingClientRect().left;
    const top = (n) => window.page(n).getBoundingClientRect().top;
    return { left1: left(1), left2: left(2), top2: top(2), top3: top(3) };
  `);
  check("turning the spread off restores one column",
    spreadOff && Math.abs(column.left1 - column.left2) <= 1 && column.top3 > column.top2, JSON.stringify(column));

  // 11. Density: the canvas is drawn for the host's page zoom too, capped at 3.
  await tab.eval("window.__pages = window.doc().querySelector('.pages'); window.ask({ type: 'viewer-density', zoom: 2 })");
  const redrawn = await until(tab, "window.doc().querySelector('.pages') !== window.__pages", 10000);
  await wait(300);
  const density = await tab.eval(`
    const canvas = window.doc().querySelector('.page canvas');
    const css = parseFloat(canvas.style.width);
    const ratio = Math.min((window.win().devicePixelRatio || 1) * 2, 3);
    return { width: canvas.width, css, expected: css * ratio };
  `);
  check("a density of 2 draws the canvas at twice the pixel ratio",
    redrawn && Math.abs(density.width / density.expected - 1) < 0.02, JSON.stringify(density));
  await tab.eval("window.__pages = window.doc().querySelector('.pages'); window.ask({ type: 'viewer-density', zoom: 1 })");
  await until(tab, "window.doc().querySelector('.pages') !== window.__pages", 10000);
  await wait(300);

  // 12. Invert.
  await tab.eval("window.ask({ type: 'viewer-invert', invert: true })");
  const inverted = await until(tab, "window.state().invert === true && window.doc().documentElement.hasAttribute('data-invert')");
  check("viewer-invert sets data-invert and reports it", inverted);
  await tab.eval("window.ask({ type: 'viewer-invert', invert: false })");
  await until(tab, "window.state().invert === false");

  // 13. Presentation.
  await tab.eval("window.__pages = window.doc().querySelector('.pages'); window.ask({ type: 'viewer-present', on: true })");
  const presenting = await until(tab, "window.state().presenting === true && window.doc().documentElement.classList.contains('presenting')");
  await until(tab, "window.doc().querySelector('.pages') !== window.__pages", 10000);
  const during = await tab.eval("return { mode: window.state().mode, spread: window.state().spread }");
  await tab.eval("window.__pages = window.doc().querySelector('.pages'); window.ask({ type: 'viewer-present', on: false })");
  const left = await until(tab, "window.state().presenting === false && !window.doc().documentElement.classList.contains('presenting')");
  await until(tab, "window.doc().querySelector('.pages') !== window.__pages", 10000);
  await wait(500);
  const restored = await tab.eval("return { mode: window.state().mode, spread: window.state().spread, sidebar: window.state().sidebar }");
  check("presentation sets the class and state, then puts the previous mode back",
    presenting && during.mode === "page-fit" && left && restored.mode === "auto" && restored.spread === false && restored.sidebar === false,
    JSON.stringify({ during, restored }));

  // 14. Ctrl+wheel zooms the page once. The commit must land on the scale the
  // preview showed: a second handler for the same gesture would compound it.
  const scaleBefore = await tab.eval("return window.state().scale");
  await mouse("mouseWheel", 500, 400, { deltaX: 0, deltaY: -30, modifiers: 2 });
  // Read at once, well inside the 200 ms (WHEEL_IDLE_MS in pinch.js) before the commit.
  const previewed = await tab.eval(`
    const main = window.doc().querySelector('main');
    const matrix = new (window.win().DOMMatrix)(window.win().getComputedStyle(main).transform);
    return matrix.a;
  `);
  const zoomed = await until(tab, `window.state().scale > ${scaleBefore} + 0.05`, 8000);
  await wait(800);
  const scaleAfter = await tab.eval("return window.state().scale");
  const ratio = scaleAfter / scaleBefore;
  check("ctrl+wheel redraws the page at the scale its preview showed", zoomed && previewed > 1 && Math.abs(ratio / previewed - 1) < 0.03,
    JSON.stringify({ before: scaleBefore, after: scaleAfter, previewed, ratio }));

  // 15. Presentation shows one page to a screen: the current page is centred
  // in the viewport, its neighbour is below the fold, and a step centres the next.
  await tab.eval("window.__pages = window.doc().querySelector('.pages'); window.ask({ type: 'viewer-present', on: true })");
  await until(tab, "window.state().presenting === true");
  await until(tab, "window.doc().querySelector('.pages') !== window.__pages", 10000);
  await wait(500);
  await tab.eval("window.ask({ type: 'viewer-page', page: 1 })");
  await wait(500);
  const slot = (n) => tab.eval(`
    const rect = window.page(${n}).getBoundingClientRect();
    const next = window.page(${n + 1});
    return { top: rect.top, height: rect.height, inner: window.win().innerHeight,
      nextTop: next ? next.getBoundingClientRect().top : null };
  `);
  const one = await slot(1);
  check("presentation centres the current page and hides its neighbour",
    Math.abs(one.top - (one.inner - one.height) / 2) < 2 && one.nextTop >= one.inner - 1, JSON.stringify(one));
  await tab.eval("window.ask({ type: 'viewer-page', page: 2 })");
  await wait(500);
  const two = await slot(2);
  check("stepping a page in presentation centres the next one",
    Math.abs(two.top - (two.inner - two.height) / 2) < 2, JSON.stringify(two));
  await tab.eval("window.ask({ type: 'viewer-present', on: false })");
  await until(tab, "window.state().presenting === false");
}

await new Promise((resolve) => server.listen(0, "localhost", resolve));
BASE = `http://localhost:${server.address().port}`;
try {
  await run();
} catch (error) {
  check("the PDF viewer features check ran", false, String(error).slice(0, 500));
} finally {
  await chrome?.close();
  if (profile) removeTemporary(profile);
  server.close();
  await wait(150);
}

for (const { what, ok, detail } of results) console.log(`pdf-viewer-features: ${ok ? "ok  " : "FAIL"}  ${what}${detail ? ` -- ${detail}` : ""}`);
if (failures) {
  console.error(`pdf-viewer-features: ${failures} of ${results.length} checks failed`);
  process.exit(1);
}
console.log(`pdf-viewer-features: ${results.length} browser checks passed`);
