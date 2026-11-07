// The PDF, drawn into the frame a document lives in.
//
// This is the rendered state of the LaTeX preview: every page's
// canvas and text layer, stacked vertically in one scrolling document, which
// is what makes the text of a LaTeX document one sequence rather than a pile
// of pages. pdf.js and its worker come from our own static assets and never
// from a CDN. The documents origin currently allows HTTPS script sources;
// serving pdf.js from the application's own static assets keeps this renderer
// pinned to the application release. Its worker runs from `blob:`.
//
// It is loaded lazily by `viewer.js`, on the first PDF that arrives. pdf.js
// is the largest thing the web build has after the typst module, and a reader
// looking at a markdown document must never pay for it.

import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { piecesOf } from "./text.js";
import { viewerScale } from "./fit.js";
import { linkLayer } from "./links.js";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

/// Where each page starts in the joined text, so the caret lock and SyncTeX
/// -- steps 3 and 6, neither built here -- can ask "which page is this
/// offset on" without walking the DOM again. Kept as the cumulative length of
/// everything this module put into the body, which is the same number the
/// agent's own table arrives at because marks add no text.
let pageStarts = [];
let renderScale = 1;

export function pageForOffset(offset) {
  if (!pageStarts.length) return -1;
  let page = 0;
  while (page + 1 < pageStarts.length && pageStarts[page + 1] <= offset) page += 1;
  return page;
}

export function pointFromClient(clientX, clientY) {
  const found = document.elementFromPoint(clientX, clientY)?.closest?.(".page[data-page]");
  const element = found instanceof HTMLElement ? found : null;
  if (!element || !renderScale) return null;
  const rect = element.getBoundingClientRect();
  return { page: Number(element.dataset.page), x: (clientX - rect.left) / renderScale,
    y: (clientY - rect.top) / renderScale };
}

export function locatePoint(page, x, y) {
  const element = document.querySelector(`.page[data-page="${Number(page)}"]`);
  if (!element || !renderScale) return false;
  const rect = element.getBoundingClientRect();
  scrollTo({ top: scrollY + rect.top + Number(y) * renderScale - innerHeight / 3, behavior: "smooth" });
  // The marker is drawn at the point's place in the document, not on screen,
  // because the scroll above is still moving when this runs. It is the
  // overlay's to draw: the page DOM must not change once it is swapped in.
  marker?.(scrollX + rect.left + Number(x) * renderScale, scrollY + rect.top + Number(y) * renderScale);
  return true;
}

let marker = null;
/// Where `locatePoint` draws its marker: `draw(documentX, documentY)`.
export function setMarker(draw) {
  marker = typeof draw === "function" ? draw : null;
}

let generation = 0;
// The frame owns one worker for its lifetime. Passing it explicitly keeps
// pdf.js from starting and terminating a worker on every preview; each
// loading task still owns and releases its own document resources.
let pdfWorker = null;

function documentWorker() {
  if (!pdfWorker || pdfWorker.destroyed) pdfWorker = new pdfjs.PDFWorker();
  return pdfWorker;
}

/// Starts the worker before there is anything to draw, so the first PDF does
/// not also wait for the worker's script to load and start.
export function warm() {
  documentWorker();
}

const loadings = new WeakMap();

function startLoading(bytes) {
  return pdfjs.getDocument({
    worker: documentWorker(),
    data: bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes),
    // No fetching of anything, from anywhere, at draw time. A PDF that names
    // a standard font gets pdf.js's own metrics; one that names a URL gets
    // nothing, which is what a document served from a sandboxed origin
    // should get.
    isEvalSupported: false,
    disableFontFace: false,
  });
}

async function closeDocument(document_, loading) {
  // Release document resources on success, failure, and generation
  // cancellation. An explicitly supplied worker survives task destruction
  // and can also serve concurrent text extraction or a newer preview.
  try {
    await document_?.destroy();
  } catch {
    /* cleanup must not mask the render result */
  }
  try {
    await loading.destroy();
  } catch {
    /* the document destroy above is sufficient on older pdf.js versions */
  }
}

/// Release a document `render` handed out. The caller does this once the
/// next document is drawn and the sidebar has moved on to it, so the
/// thumbnails and the outline never read from a destroyed document.
export async function discard(document_) {
  const loading = loadings.get(document_);
  loadings.delete(document_);
  if (loading) await closeDocument(document_, loading);
}

/// Open `bytes` as a pdf.js document, run `fn` against it, and release the
/// document afterward. `fn` receives the resolved document and its
/// result becomes this function's result; whatever `fn` throws or returns
/// early propagates the same way, since the teardown lives in `finally` and
/// runs on every path.
async function withDocument(bytes, fn) {
  const loading = startLoading(bytes);
  let document_ = null;
  try {
    document_ = await loading.promise;
    return await fn(document_);
  } finally {
    await closeDocument(document_, loading);
  }
}

/// Draw `bytes` into `root`, replacing whatever was there.
///
/// A second `preview` arrives on every recompile, so this has to be safe to
/// call again while the last one is still rendering: `generation` is the
/// token that lets a superseded run drop its pages on the floor instead of
/// appending them under the new document's.
///
/// `options` is `{ mode, spread, sidebarWidth, presenting, density, ongo,
/// beforeswap, afterswap }`. The result is `null` when a newer render won, otherwise
/// `{ pages, document, sizes, rastered }` with the page sizes in PDF points.
/// Every page is in the document when this returns, at its exact size and
/// carrying its text, but only the pages in or near view have their canvas,
/// selectable text spans and links: the rest are built as they approach the
/// viewport. `rastered` settles when the pages near the viewport after the
/// swap are built. The document is left open for the sidebar and for the
/// lazy pages; `discard` releases it. A superseded or failed render
/// releases its own.
///
/// `beforeswap()` runs just before the single swap and returns whatever the
/// caller needs to keep the reading position; `afterswap` gets it back
/// straight after, in the same task, so no frame is painted between.
export async function render(bytes, root, options = {}) {
  const { mode = "auto", spread = false, sidebarWidth = 0, presenting = false, density = 1,
    ongo = () => {}, beforeswap = null, afterswap = null } = options;
  const mine = ++generation;
  const loading = startLoading(bytes);
  let document_ = null;
  let handedOn = false;
  const entries = [];
  const stale = () => mine !== generation;
  try {
    document_ = await loading.promise;
    if (stale()) return null;

    // Everything is built off-document and swapped in at the end. Painting page
    // by page into the live body would have the agent's observer republish a
    // half-drawn document once per page, and the sidebar re-anchor every
    // comment against text that is about to grow.
    const staging = document.createElement("div");
    staging.className = spread ? "pages spread" : "pages";
    const starts = [];
    let offset = 0;

    // Every page is drawn at one scale, chosen from the widest page: a
    // document whose pages disagree about their size is still one column of
    // pages, and rescaling halfway down it would read as a mistake, but a
    // later page wider than the first must not be the one that overflows.
    const sizes = [];
    const pages = [];
    for (let number = 1; number <= document_.numPages; number++) {
      const page = await document_.getPage(number);
      if (stale()) return null;
      const size = page.getViewport({ scale: 1 });
      sizes.push({ width: size.width, height: size.height });
      pages.push(page);
    }
    const widest = Math.max(1, ...sizes.map((size) => size.width));
    const tallest = Math.max(1, ...sizes.map((size) => size.height));
    // The frame is all page now: the controls moved out to the preview
    // header, so nothing is subtracted for a bar inside it.
    const scale = viewerScale(mode, document.documentElement.clientWidth,
      document.documentElement.clientHeight, widest, tallest,
      { columns: spread ? 2 : 1, sidebar: sidebarWidth, bleed: presenting });
    renderScale = scale;
    staging.dataset.scale = String(scale / (96 / 72));

    // The agent publishes the text of the whole document, so every page's
    // text content is read (in the worker, a few pages at a time) even though
    // only some pages get their span layer built.
    const contents = new Array(pages.length);
    const BATCH = 8;
    for (let from = 0; from < pages.length; from += BATCH) {
      await Promise.all(pages.slice(from, from + BATCH).map(async (page, i) => {
        contents[from + i] = await page.getTextContent();
      }));
      if (stale()) return null;
    }

    for (let number = 1; number <= pages.length; number++) {
      const page = pages[number - 1];
      const viewport = page.getViewport({ scale });

      const frame = document.createElement("div");
      frame.className = "page";
      frame.dataset.page = String(number);
      frame.style.width = `${Math.floor(viewport.width)}px`;
      frame.style.height = `${Math.floor(viewport.height)}px`;
      // Presentation centres the page in a slot one viewport high from this.
      frame.style.setProperty("--page-height", `${Math.floor(viewport.height)}px`);
      // The text layer's spans size themselves from these, which is how the
      // selection overlay stays on top of the glyphs at any scale.
      frame.style.setProperty("--scale-factor", String(scale));
      frame.style.setProperty("--user-unit", "1");
      frame.style.setProperty("--total-scale-factor", String(scale));
      frame.style.setProperty("--scale-round-x", "1px");
      frame.style.setProperty("--scale-round-y", "1px");

      // Until the page is built, its text is one hidden node: the same string
      // the span layer will hold, so the document's text, its offsets and the
      // anchors made against them do not depend on which pages are built.
      const joined = joinedText(contents[number - 1].items, number > 1);
      const layer = document.createElement("div");
      layer.className = "textLayer";
      if (joined) layer.append(gap(joined));
      frame.append(layer);
      staging.append(frame);

      starts.push(offset);
      offset += joined.length;

      entries.push({ number, page, frame, viewport, content: contents[number - 1], built: false,
        building: false, wanted: false, task: null, height: viewport.height });
    }

    // Only the pages that will be in view after the swap are built before
    // it, plus one either side. The rest are sized frames: drawing into them
    // later changes no text. The anchor is read once here to choose them and
    // again at the swap to restore the position, because the reader may have
    // scrolled meanwhile.
    const hint = beforeswap?.();
    const target = Math.min(Math.max(1, hint?.page ?? 1), entries.length);
    const wanted = document.documentElement.clientHeight * (spread ? 2 : 1);
    const early = new Set();
    if (target > 1) early.add(target - 1);
    for (let number = target, height = 0; number <= entries.length && height < wanted; number++) {
      early.add(number);
      height += entries[number - 1].height;
    }
    const last = Math.max(...early);
    if (last < entries.length) early.add(last + 1);
    const ctx = { document: document_, ongo, stale, ratio: Math.min((globalThis.devicePixelRatio || 1) * density, 3) };
    for (const number of [...early].sort((a, b) => a - b)) {
      await buildPage(entries[number - 1], ctx);
      if (stale()) return null;
    }

    const anchor = beforeswap?.();
    // The previous document's lazy work stops here, before its pages go.
    stopLazy();
    root.replaceChildren(staging);
    pageStarts = starts;
    afterswap?.(anchor);
    loadings.set(document_, loading);
    handedOn = true;
    const rastered = startLazy(entries, ctx);
    return { pages: document_.numPages, document: document_, sizes, rastered };
  } finally {
    if (!handedOn) {
      for (const entry of entries) entry.task?.cancel();
      await closeDocument(document_, loading);
    }
  }
}

/// Lazy building of the pages that were not ready at the swap.
let lazy = null;

function stopLazy() {
  lazy?.stop();
  lazy = null;
}

/// Build the remaining pages as they come within about a viewport of the
/// screen. Returns a promise that settles when the pages near the viewport
/// have been built (or the document was superseded); it never rejects.
function startLazy(entries, ctx) {
  const queue = [];
  const byFrame = new Map(entries.map((entry) => [entry.frame, entry]));
  let stopped = false;
  let pumping = false;
  /** @type {(value?: unknown) => void} */
  let resolveFirst = () => {};
  const first = new Promise((resolve) => { resolveFirst = resolve; });
  const gone = () => stopped || ctx.stale();

  async function pump() {
    if (pumping) return;
    pumping = true;
    try {
      while (!gone()) {
        const entry = queue.shift();
        if (!entry) break;
        if (entry.built || entry.building || !entry.wanted) continue;
        try {
          await buildPage(entry, { ...ctx, stale: gone });
        } catch {
          /* a page that cannot be built stays a sized, blank page */
        }
      }
    } finally {
      pumping = false;
      resolveFirst();
    }
  }

  const todo = entries.filter((entry) => !entry.built);
  let observer = null;
  if (todo.length === 0) {
    resolveFirst();
  } else {
    try {
      // `root: document` makes the margin apply inside a frame whose parent
      // is another origin, where the implicit root would ignore it.
      observer = new IntersectionObserver((changes) => {
        for (const change of changes) {
          const entry = byFrame.get(change.target);
          if (!entry || entry.built) continue;
          entry.wanted = change.isIntersecting;
          if (change.isIntersecting && !queue.includes(entry)) queue.push(entry);
        }
        void pump();
      }, { root: document, rootMargin: "100% 0px 100% 0px" });
      for (const entry of todo) observer.observe(entry.frame);
    } catch {
      // No observer to wait on: build the rest in idle time, in order.
      for (const entry of todo) { entry.wanted = true; queue.push(entry); }
      const idle = globalThis.requestIdleCallback ?? ((fn) => setTimeout(fn, 50));
      idle(() => { void pump(); });
    }
  }

  lazy = {
    stop() {
      stopped = true;
      observer?.disconnect();
      for (const entry of entries) entry.task?.cancel();
      queue.length = 0;
      resolveFirst();
    },
  };
  return first;
}

/// Make one page complete: its canvas drawn, its selectable text spans in
/// place of the hidden text node, and its links. The page is inserted into
/// the frame only once drawn, so a page is never seen half-built.
async function buildPage(entry, ctx) {
  if (entry.built || entry.building) return;
  entry.building = true;
  const { frame, page, viewport, number } = entry;
  const { ratio, stale } = ctx;
  try {
    const canvas = document.createElement("canvas");
    canvas.width = Math.floor(viewport.width * ratio);
    canvas.height = Math.floor(viewport.height * ratio);
    // WebKit leaves page zoom out of devicePixelRatio, so in the desktop app
    // at 125% the canvas would be drawn for 100% and stretched. The host
    // tells the frame the zoom as `density` and it multiplies in here. The
    // cap bounds the canvas memory of a large page on a dense screen.
    canvas.style.width = `${Math.floor(viewport.width)}px`;
    canvas.style.height = `${Math.floor(viewport.height)}px`;
    try {
      entry.task = page.render({
        canvas,
        canvasContext: canvas.getContext("2d"),
        viewport,
        transform: ratio === 1 ? null : [ratio, 0, 0, ratio, 0, 0],
      });
      await entry.task.promise;
    } catch {
      /* cancelled, or a page that cannot be drawn: blank, but still a page */
    }
    entry.task = null;
    if (stale()) return;
    frame.prepend(canvas);

    const layer = document.createElement("div");
    layer.className = "textLayer";
    const text = new pdfjs.TextLayer({ textContentSource: entry.content, container: layer, viewport });
    await text.render();
    if (stale()) return;
    rewrite(text.textDivs, entry.content.items, number > 1);
    frame.querySelector(".textLayer")?.replaceWith(layer);

    // The links come after the text, so that nothing they hold could ever
    // count as text. A page whose annotations cannot be read is still a page.
    try {
      const links = await linkLayer(page, viewport, ctx.document, ctx.ongo);
      if (stale()) return;
      frame.append(links);
    } catch {
      /* no links */
    }
    entry.built = true;
    entry.content = null;
    page.cleanup();
  } finally {
    entry.building = false;
  }
}

/// Extract the same normalized visible text the viewer publishes, without
/// creating a canvas or requiring a LaTeX compiler. Historical passage lookup
/// uses this for PDFs already stored by an editor.
export async function text(bytes) {
  return withDocument(bytes, async (document_) => {
    const pages = [];
    for (let number = 1; number <= document_.numPages; number++) {
      const page = await document_.getPage(number);
      try {
        const content = await page.getTextContent();
        pages.push(runsOf(content.items));
      } finally {
        page.cleanup();
      }
    }
    return piecesOf(pages)
      .map((piece) => piece.text)
      .join("");
  });
}

/// Map pdf.js text-content items to the run shape `piecesOf` consumes.
///
/// pdf.js emits marked-content markers alongside real text items, and those
/// markers carry no `str`; the filter drops them so `piecesOf` only ever
/// sees runs with actual text.
function runsOf(items) {
  return items
    .filter((item) => item.str !== undefined)
    .map((item) => ({
      text: item.str,
      left: item.transform[4],
      width: item.width,
      height: Math.hypot(item.transform[2], item.transform[3]) || item.height || 1,
      eol: Boolean(item.hasEOL),
    }));
}

/// The text a page holds once built, as one string, without any DOM: what
/// `rewrite` leaves in the span layer. A page that is not built yet carries
/// this as a single hidden node, so the published text is the same whether
/// or not the page has been reached.
function joinedText(items, leadingBreak) {
  const pieces = piecesOf([runsOf(items)]);
  if (!pieces.some((piece) => piece.kind !== "gap")) return "";
  return (leadingBreak ? "\n\n" : "") + pieces.map((piece) => piece.text).join("");
}

/// Turn pdf.js's spans into something the agent can read as prose.
///
/// `textDivs` is one span per text item, in reading order, and it lines up
/// with the items pdf.js kept -- the ones with a `str`; the rest are marked
/// content markers, which produce no span. `piecesOf` decides what belongs
/// between them; this puts those decisions in the DOM.
///
/// Returns how many characters of joined text this page contributed, so the
/// caller can keep the page-start table.
function rewrite(textDivs, items, leadingBreak) {
  const runs = runsOf(items);
  // `piecesOf` puts the blank line between pages; here each page is rewritten
  // on its own, so it is asked for one page and the break is added by hand.
  const pieces = piecesOf([runs]);

  let length = 0;
  if (leadingBreak) {
    // The gap between two pages. It lives in the earlier page's own layer so
    // that a page removed takes its separator with it.
    const first = textDivs[0];
    if (first) {
      first.before(gap("\n\n"));
      length += 2;
    }
  }
  let at = 0; // which span the next "run" piece belongs to
  let last = null;
  for (const piece of pieces) {
    if (piece.kind === "gap") {
      if (last) last.after(gap(piece.text));
      length += piece.text.length;
      continue;
    }
    const div = textDivs[at++];
    // pdf.js includes detached spans for empty end-of-line items. Keep the
    // last attached run as the insertion point, or the following newline
    // is inserted beside a detached span and disappears from the text layer.
    if (!div?.parentNode) continue;
    // The span's own text, folded and with a line-break hyphen taken off.
    // The hyphen goes back as generated content: `content` is drawn but is
    // not a text node, so the reader sees `inter-` at the line end and the
    // agent joins `interval`.
    if (div.textContent !== piece.text) div.textContent = piece.text;
    div.classList.toggle("hyphen", Boolean(piece.hyphen));
    length += piece.text.length;
    last = div;
  }
  return length;
}

/// A separator between runs. It has to be a text node the agent's walk sees,
/// and it must not be visible: an absolutely-positioned, zero-sized block at
/// the page's origin is both. `white-space: pre` on the text layer's spans is
/// what keeps a newline from collapsing before the agent reads it.
function gap(text) {
  const span = document.createElement("span");
  span.className = "gap";
  span.textContent = text;
  return span;
}
