// The PDF frame.
//
// A LaTeX document is a PDF, so the frame the reader paints into cannot be
// the empty HTML shell every other format gets: there is no HTML to paint.
// This page is that frame. It is served on the documents origin with the same
// CSP, the same `frame-ancestors` and the same agent as `serve_shell` gives a
// document, and it takes the same `preview` message the editor already sends
// on every recompile -- with `pdf` bytes where an HTML document has `html`.
//
// Nothing is drawn until a message arrives. A reader who opens this page
// directly is looking at a frame with no document in it, and it says so
// rather than sitting blank.
//
// pdf.js is loaded here and only here. The import is dynamic so the bytes
// are fetched on the first PDF and never by the reader shell, which has no
// use for them.

import { watchPinch } from "../lib/pinch.js";
import { createSidebar, SIDEBAR_WIDTH } from "../lib/pdf/sidebar.js";
import { createFind } from "../lib/pdf/find.js";
import { installInput } from "../lib/pdf/input.js";
import { GUTTER } from "../lib/pdf/fit.js";

let viewer = null; // the render module, once something has needed it
let stage = null;
let note = null;
let pan = null;
let scaleMode = "auto";
let sidebarOpen = false;
let spread = false;
let invert = false;
let presenting = false;
let density = 1; // the host's page zoom, which WebKit leaves out of devicePixelRatio
let saved = null; // what presentation took over, put back when it ends
let findQuery = "";
let findState = null;
let pageCount = 0;
let currentPage = 0;
let liveDocument = null; // the pdf.js document the sidebar and links are reading
let painting = false;
// The reading position a redraw must come back to when the caller knows
// better than the default (zoom at the cursor, a presentation page).
let forcedAnchor = null;
// `with_frame()` injects the same configured frame.js tag into this page as
// it does into rendered documents. Pin the parent origin from that trusted
// tag; learning it from the first message would let an arbitrary embedder
// claim this viewer before the real reader does.
let readerOrigin = "";
try {
  const agent = [...document.scripts].find((script) => new URL(script.src, document.baseURI).pathname.endsWith("/frame.js"));
  const configuredReader = agent && new URL(agent.src, document.baseURI).searchParams.get("reader");
  readerOrigin = configuredReader ? new URL(configuredReader).origin : "";
} catch {
  readerOrigin = "";
}
let paintGeneration = 0;
// The last PDF drawn, kept so a resize can redraw it. The pages are sized to
// the width the frame has (see `pdf/render.js`), and the frame's width is not
// fixed: the pane beside the source is dragged, the layout button gives the
// document the whole window, and a phone is turned on its side. Without this
// the page keeps whatever width it was first drawn at and is clipped or
// stranded in the middle of the frame.
let drawn = null;

// The sidebar and the overlay are hosts with shadow roots, appended once and
// never replaced: the agent walks every text node in the body, and neither
// the thumbnails' captions nor a transient marker may be in its way. Only the
// stage and the note below come and go.
const sidebar = createSidebar({ ongo: go });
document.body.append(sidebar.element);
const overlay = createOverlay();

const reducedMotion = () => Boolean(matchMedia?.("(prefers-reduced-motion: reduce)").matches);

/// One shadow-rooted host at the document origin that transient markers are
/// drawn into. Styled through the CSSOM and animated through the Web
/// Animations API, so no stylesheet is injected into the page.
function createOverlay() {
  const host = document.createElement("div");
  host.style.cssText = "position:absolute;left:0;top:0;width:0;height:0;overflow:visible;pointer-events:none;z-index:40000";
  const root = host.attachShadow({ mode: "closed" });
  document.body.append(host);
  return {
    /// A ring at a point of the document, for about 1.2 seconds.
    flash(x, y) {
      const ring = document.createElement("div");
      ring.style.cssText = `position:absolute;left:${x - 16}px;top:${y - 16}px;width:32px;height:32px;`
        + "box-sizing:border-box;border:3px solid var(--lp-marker);border-radius:50%;";
      root.append(ring);
      if (!reducedMotion() && ring.animate) {
        ring.animate([{ opacity: 1, transform: "scale(0.6)" }, { opacity: 1, transform: "scale(1)", offset: 0.4 },
          { opacity: 0, transform: "scale(1.3)" }], { duration: 1200, easing: "ease-out", fill: "forwards" });
      }
      setTimeout(() => ring.remove(), 1200);
    },
  };
}

function ready() {
  if (stage) return stage;
  note?.remove();
  note = null;
  stage = document.createElement("main");
  document.body.prepend(stage);
  watchText(stage);
  return stage;
}

// Comment marks are painted into the text layers by splitting and merging text
// nodes, which collapses the Ranges the search highlights hold. So a structural
// change under the stage while a search is active has the highlights rebuilt,
// once per burst. `find.refresh` only touches CSS.highlights, so it cannot
// trip this observer again. The swap in `render` is skipped: `paint` refreshes
// after it.
let textObserver = null;
let refreshQueued = false;
function watchText(target) {
  textObserver?.disconnect();
  textObserver = new MutationObserver(() => {
    if (!findQuery || painting || refreshQueued) return;
    refreshQueued = true;
    requestAnimationFrame(() => {
      refreshQueued = false;
      if (findQuery && !painting) find.refresh();
    });
  });
  textObserver.observe(target, { childList: true, subtree: true, characterData: true });
}

/// What the frame has nothing to show. Two sentences at most, in the same
/// voice as the reader's own "Not yet rendered" card, because a reader who
/// sees this cannot tell which side of the frame boundary wrote it.
function waiting(title, detail = "") {
  const element = document.createElement("div");
  element.className = "note";
  const heading = document.createElement("p");
  heading.className = "note-title";
  heading.textContent = title;
  element.append(heading);
  if (detail) {
    const line = document.createElement("p");
    line.textContent = detail;
    element.append(line);
  }
  pan?.select();
  textObserver?.disconnect();
  textObserver = null;
  stage?.remove();
  note?.remove();
  note = element;
  document.body.prepend(note);
  stage = null;
  drawn = null;
  pageCount = 0;
  currentPage = 0;
  findState = null;
  find.clear();
  void sidebar.setDocument(null);
  retire(null);
  // The host ends its fullscreen on its own; what presentation took over is
  // put back so the next document does not open in page-fit.
  if (presenting) {
    presenting = false;
    if (saved) ({ mode: scaleMode, spread, sidebarOpen } = saved);
    saved = null;
  }
  applyChrome();
  report();
}

/// Destroy the previous pdf.js document once nothing reads from it.
function retire(next) {
  const previous = liveDocument;
  liveDocument = next;
  if (previous && previous !== next) void viewer?.discard(previous).catch(() => {});
}

const sidebarOffset = () => (sidebarOpen && !presenting && drawn ? SIDEBAR_WIDTH : 0);

/// Everything the page column has to agree with before a draw: the sidebar's
/// visibility and the room it takes, and the presentation class.
function applyChrome() {
  sidebar.setOpen(Boolean(sidebarOpen && !presenting && drawn));
  document.documentElement.style.setProperty("--lp-sidebar-offset", `${sidebarOffset()}px`);
  document.documentElement.classList.toggle("presenting", presenting);
}

let lastReport = "";
/// Tell the header what the page is drawn at and what the frame is showing.
///
/// It owns the zoom control, so it cannot show "150%" beside a page it did
/// not measure: the scale that came out of `viewerScale` is this side's to
/// report. No PDF on screen is reported too -- that is what takes the
/// controls out of the header. Only a changed state is sent, except when the
/// header has just asked.
function report(force = false) {
  if (!readerOrigin) return;
  const scale = stage?.firstElementChild ? Number(stage.firstElementChild.dataset.scale) : null;
  const state = { librepaper: true, type: "viewer-state", drawn: Boolean(drawn), mode: scaleMode,
    scale: Number.isFinite(scale) ? scale : null, page: drawn ? currentPage : 0, pages: drawn ? pageCount : 0,
    sidebar: sidebarOpen, spread, invert, presenting, find: findState };
  const key = JSON.stringify(state);
  if (!force && key === lastReport) return;
  lastReport = key;
  parent.postMessage(state, readerOrigin);
}

const find = createFind({
  root: () => stage?.querySelector(".pages") ?? null,
  onresult({ current, total }) {
    findState = findQuery ? { current: Math.max(0, Math.trunc(current) || 0), total: Math.max(0, Math.trunc(total) || 0) } : null;
    report();
  },
});

/// Where a client point sits on the pages: the nearest page and how far
/// across and down it, as fractions. Fractions survive a change of scale, a
/// new column count and a new document of the same length.
function captureAnchor(x, y) {
  let best = null;
  let bestDistance = Infinity;
  for (const element of stage?.querySelectorAll(".page") ?? []) {
    const rect = element.getBoundingClientRect();
    const dx = x < rect.left ? rect.left - x : x > rect.right ? x - rect.right : 0;
    const dy = y < rect.top ? rect.top - y : y > rect.bottom ? y - rect.bottom : 0;
    const distance = dx * dx + dy * dy;
    if (distance < bestDistance && rect.width > 0 && rect.height > 0) {
      bestDistance = distance;
      best = { page: Number(element.dataset.page), fx: (x - rect.left) / rect.width, fy: (y - rect.top) / rect.height, x, y };
      if (distance === 0) break;
    }
  }
  return best;
}

/// Scroll so the anchored spot of its page is back under the client point.
function restoreAnchor(anchor) {
  if (!anchor) return;
  const rect = stage?.querySelector(`.page[data-page="${anchor.page}"]`)?.getBoundingClientRect();
  if (!rect) return;
  window.scrollBy(rect.left + anchor.fx * rect.width - anchor.x, rect.top + anchor.fy * rect.height - anchor.y);
}

/// The top-centre of a page, as an anchor for presentation.
function pageTopAnchor(page) {
  const offset = sidebarOffset();
  return { page, fx: 0.5, fy: 0, x: offset + (document.documentElement.clientWidth - offset) / 2, y: 0 };
}

/// The pages at least fit across the frame: there is nothing to scroll
/// sideways, so the root says so and a stray wheel delta cannot shake the page.
function settleOverflow() {
  const pages = stage?.querySelector(".pages");
  if (!pages) return;
  const room = document.documentElement.clientWidth - sidebarOffset();
  const fits = pages.getBoundingClientRect().width + (presenting ? 0 : GUTTER) <= room + 0.5;
  document.documentElement.style.overflowX = fits ? "hidden" : "";
}

/// Runs inside `render`, straight after the swap.
function settle(anchor) {
  clearPreview();
  zoom = null;
  forcedAnchor = null;
  applyChrome();
  settleOverflow();
  restoreAnchor(anchor);
}

/// Draw `bytes`, which may be the ones already on screen.
///
/// One path for a new preview and for a redraw, so the two cannot drift about
/// what `librepaperViewer` holds or which generation won.
async function paint(bytes) {
  const mine = ++paintGeneration;
  // pdf.js hands the buffer to its worker, which detaches it. So `drawn` is
  // taken first and every draw is given a copy of it: what is kept has to
  // outlive the draw that is about to eat it, redraws included.
  const keep = ArrayBuffer.isView(bytes)
    ? new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength).slice()
    : new Uint8Array(bytes).slice();
  drawn = keep;
  painting = true;
  lastWidth = document.documentElement.clientWidth;
  lastHeight = document.documentElement.clientHeight;
  try {
    const first = !viewer;
    viewer ??= await import("../lib/pdf/render.js");
    if (first) viewer.setMarker((x, y) => overlay.flash(x, y));
    // Several previews can arrive while the module loads. Only the newest
    // may start drawing, so an older import continuation cannot cancel it.
    if (mine !== paintGeneration) return;
    applyChrome();
    const result = await viewer.render(keep.slice(), ready(), {
      mode: scaleMode, spread, sidebarWidth: sidebarOffset(), presenting, density, ongo: go,
      beforeswap: () => forcedAnchor ?? captureAnchor(sidebarOffset(), 0),
      afterswap: settle,
    });
    // A render that was superseded has already released its document. One
    // that came back has already swapped its pages in, so it is adopted even
    // if a newer paint has started since: the pages on screen are its pages.
    if (!result) return;
    pageCount = result.pages;
    // The sidebar moves to the new document before the old one goes away.
    sidebar.setDocument(result.document)?.catch?.(() => {});
    retire(result.document);
    currentPage = measureCurrent();
    sidebar.setCurrent(currentPage);
    if (findQuery) find.refresh();
    if (mine !== paintGeneration) return;
    painting = false;
    report();
    // The page index for an offset, for the caret lock and SyncTeX. Neither
    // is built here; both need this and nothing else from the viewer, so it
    // is exposed now rather than left for them to reach into the DOM for.
    globalThis.librepaperViewer = { pageForOffset: viewer.pageForOffset,
      pointFromClient: viewer.pointFromClient, locatePoint: viewer.locatePoint, pages: result.pages };
    // The agent republishes off its own mutation observer, which the swap in
    // `render` has just tripped: one code path for "the document changed",
    // whether the change was an HTML preview or a page finishing. There is
    // deliberately no second signal here.
  } catch (error) {
    if (mine !== paintGeneration) return;
    painting = false;
    waiting("This PDF could not be drawn.", String(error?.message || error));
  }
}

const redraw = () => {
  if (drawn) void paint(drawn);
};

// A drag of the pane separator is a stream of resizes and each redraw is a
// full rasterisation, so the redraw waits for the drag to stop. A width that
// comes back to where it started asks for no work at all.
let lastWidth = 0;
let lastHeight = 0;
let resizeTimer = 0;
addEventListener("resize", () => {
  if (!drawn) return;
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    const width = document.documentElement.clientWidth;
    const height = document.documentElement.clientHeight;
    if (!width || (width === lastWidth && height === lastHeight)) return;
    lastHeight = height;
    lastWidth = width;
    redraw();
  }, 150);
});

/// Take the user to a page, and to a point on it when the destination has one.
/// A thumbnail, an outline entry and a link all end up here.
function go({ page, y }) {
  const element = stage?.querySelector(`.page[data-page="${Math.trunc(Number(page))}"]`);
  if (!element) return;
  const rect = element.getBoundingClientRect();
  const scale = Number(stage.firstElementChild?.dataset.scale) * (96 / 72);
  const inside = !presenting && typeof y === "number" && Number.isFinite(y) && scale > 0 ? y * scale - 16 : 0;
  scrollToTop(scrollY + rect.top - slotMargin(rect) + Math.max(0, inside));
  currentPage = Number(element.dataset.page);
  sidebar.setCurrent(currentPage);
  report();
}

/// How far above a page's top edge the scroll should land. In presentation
/// each page sits centred in a slot one viewport high, so the slot starts
/// above the page by the margin that centres it; elsewhere the page is the top.
function slotMargin(rect) {
  return presenting ? Math.max(0, (innerHeight - rect.height) / 2) : 0;
}

function scrollToTop(top) {
  scrollTo({ top, behavior: presenting || reducedMotion() ? "auto" : "smooth" });
}

function pageElements() {
  return [...(stage?.querySelectorAll(".pages > .page") ?? [])];
}

/// The page under a point a third of the way down the viewport; in a spread,
/// the left page of its row.
function measureCurrent() {
  const pages = stage?.querySelector(".pages")?.children;
  if (!pages?.length) return 0;
  const y = innerHeight / 3;
  let low = 0;
  let high = pages.length - 1;
  let found = 0;
  while (low <= high) {
    const middle = (low + high) >> 1;
    if (pages[middle].getBoundingClientRect().top <= y) {
      found = middle;
      low = middle + 1;
    } else {
      high = middle - 1;
    }
  }
  const top = pages[found].getBoundingClientRect().top;
  while (found > 0 && Math.abs(pages[found - 1].getBoundingClientRect().top - top) < 1) found -= 1;
  return Number(pages[found].dataset.page);
}

let currentTimer = 0;
addEventListener("scroll", () => {
  if (currentTimer || !stage) return;
  currentTimer = setTimeout(() => {
    currentTimer = 0;
    const page = measureCurrent();
    if (page === currentPage) return;
    currentPage = page;
    sidebar.setCurrent(page);
    report();
  }, 100);
}, { passive: true });

function pageStep(delta) {
  const list = pageElements();
  if (!list.length) return;
  let index = Math.min(list.length - 1, Math.max(0, currentPage - 1));
  if (presenting || !spread) {
    index += delta;
  } else {
    // A row at a time: the rows are the elements that share a top edge.
    const sameRow = (a, b) => Math.abs(list[a].getBoundingClientRect().top - list[b].getBoundingClientRect().top) < 1;
    if (delta > 0) {
      const from = index;
      while (index < list.length && sameRow(from, index)) index += 1;
    } else {
      index -= 1;
      const from = index;
      while (index > 0 && from >= 0 && sameRow(from, index - 1)) index -= 1;
    }
  }
  if (index < 0 || index >= list.length) return;
  focusPage(list, index);
}

function pageEdge(which) {
  const list = pageElements();
  if (list.length) focusPage(list, which === "last" ? list.length - 1 : 0);
}

function focusPage(list, index) {
  const rect = list[index].getBoundingClientRect();
  scrollToTop(scrollY + rect.top - slotMargin(rect));
  // Taken now rather than when the scroll has finished, so keys pressed in a
  // row each step from the page the last one chose.
  currentPage = Number(list[index].dataset.page);
  sidebar.setCurrent(currentPage);
  report();
}

// Zoom. A gesture is previewed with a transform while it runs and committed as
// a redraw at the new numeric scale when it ends, anchored at the cursor: the
// pinch on a phone, ctrl+wheel and the keyboard share this.
let zoom = null; // { anchor } for the gesture in progress
function clearPreview() {
  if (!stage) return;
  stage.style.transform = "";
  stage.style.transformOrigin = "";
}

/// The scale a zoom factor multiplies: the one on screen, or the one already
/// asked for when a redraw is still running.
function zoomBase() {
  const asked = Number(scaleMode);
  if (painting && Number.isFinite(asked)) return asked;
  return Number(stage?.firstElementChild?.dataset.scale);
}

function zoomPreview(factor, x, y) {
  if (!stage?.firstElementChild || !drawn) return;
  if (!zoom) {
    const rect = stage.getBoundingClientRect();
    zoom = { anchor: captureAnchor(x, y) };
    stage.style.transformOrigin = `${x - rect.left}px ${y - rect.top}px`;
  }
  const base = zoomBase();
  const limited = Number.isFinite(base) && base > 0 ? Math.min(10 / base, Math.max(0.1 / base, factor)) : factor;
  stage.style.transform = `scale(${limited})`;
}

function zoomCommit(factor, x, y) {
  const current = zoomBase();
  const next = Math.min(10, Math.max(0.1, Number((current * factor).toFixed(2))));
  if (!drawn || !Number.isFinite(current) || Math.abs(next - current) < 0.01) {
    clearPreview();
    zoom = null;
    return;
  }
  forcedAnchor = zoom?.anchor ?? captureAnchor(x, y);
  scaleMode = String(next);
  report();
  redraw();
}

function zoomStep(factor) {
  if (!stage?.firstElementChild || !drawn) return;
  zoom = null;
  clearPreview();
  const offset = sidebarOffset();
  zoomCommit(factor, offset + (document.documentElement.clientWidth - offset) / 2, innerHeight / 2);
}

function zoomReset() {
  scaleMode = "auto";
  forcedAnchor = null;
  zoom = null;
  report();
  redraw();
}

watchPinch(document, {
  onchange: zoomPreview,
  onend: zoomCommit,
});

installInput({
  zoomStep,
  zoomReset,
  pageStep,
  pageEdge,
  onfind() {
    if (readerOrigin) parent.postMessage({ librepaper: true, type: "viewer-find-open" }, readerOrigin);
  },
  isPresenting: () => presenting,
});

// Presentation is the host's fullscreen: a postMessage carries no user
// activation, so the frame cannot ask for it. The host fullscreens its iframe
// on the click and says here when it enters and leaves (Escape included).
// Entering draws the current page to fit the screen, alone, and leaving puts
// back what the reader had. The host's fullscreen also resizes the frame; the
// resize handler compares against the size `paint` recorded, so it finds
// nothing left to do once this redraw has started.
function setPresenting(on) {
  if (on === presenting) return;
  const page = currentPage;
  if (on) {
    saved = { mode: scaleMode, spread, sidebarOpen };
    presenting = true;
    scaleMode = "page-fit";
    spread = false;
    sidebarOpen = false;
  } else {
    presenting = false;
    if (saved) ({ mode: scaleMode, spread, sidebarOpen } = saved);
    saved = null;
  }
  applyChrome();
  forcedAnchor = page ? pageTopAnchor(page) : null;
  report();
  redraw();
}

// Report scroll position changes so the Reader can hide its top bar on
// phones when the user scrolls down and reveal it when they scroll up. The
// document preview is a cross-origin iframe, so its scroll events never
// reach the Reader; this listener sees all inner element scrolls.
const scrollOffsets = new WeakMap();
let scrollRAFHandle = null;
let pendingScrollDelta = 0;
let pendingScrollTop = 0;
let pendingScrollBottom = Infinity;
function onScroll(event) {
  if (!readerOrigin) return;
  const target = event.target;
  let top = null;
  let bottom = Infinity;
  if (target === document) {
    const el = document.scrollingElement || document.documentElement;
    top = el.scrollTop;
    bottom = el.scrollHeight - el.clientHeight - el.scrollTop;
  } else if (target instanceof Element) {
    top = target.scrollTop;
    bottom = target.scrollHeight - target.clientHeight - target.scrollTop;
  }
  if (top === null) return;

  const key = target === document ? document : target;
  const previous = scrollOffsets.get(key) ?? top;
  scrollOffsets.set(key, top);
  const delta = top - previous;
  if (delta === 0) return;

  pendingScrollDelta += delta;
  pendingScrollTop = top;
  pendingScrollBottom = bottom;
  if (scrollRAFHandle === null) {
    scrollRAFHandle = requestAnimationFrame(() => {
      scrollRAFHandle = null;
      parent.postMessage({
        librepaper: true,
        type: "scroll",
        delta: Math.round(pendingScrollDelta),
        top: Math.round(pendingScrollTop),
        bottom: Math.round(pendingScrollBottom),
      }, readerOrigin);
      pendingScrollDelta = 0;
    });
  }
}
document.addEventListener("scroll", onScroll, { capture: true, passive: true });

addEventListener("message", async (event) => {
  if (!readerOrigin || event.source !== parent || event.origin !== readerOrigin) return;
  const message = event.data;
  if (!message || message.librepaper !== true) return;

  // Zoom, from the header's controls. A mode is redrawn at once, because the
  // pages on screen are the wrong size the moment it changes.
  if (message.type === "viewer-scale") {
    if (presenting || !validMode(message.mode)) return;
    scaleMode = String(message.mode);
    forcedAnchor = null;
    if (drawn) await paint(drawn);
    else report();
    return;
  }
  // The reader's colour mode. Nothing is redrawn: the pages are the document
  // and stay as rendered, only the ground and the notes around them change.
  if (message.type === "theme") {
    if (message.mode === "light" || message.mode === "dark") {
      document.documentElement.dataset.mode = message.mode;
    }
    return;
  }
  // The cursor tool, likewise. pdf.js's grab-to-pan is the only piece of the
  // old in-frame toolbar that has to live here.
  if (message.type === "viewer-tool") {
    if (message.tool !== "hand" && message.tool !== "select") return;
    pan ??= (await import("../lib/pdf/pan.js")).createPan();
    message.tool === "hand" ? pan.grab() : pan.select();
    return;
  }
  if (message.type === "viewer-page") {
    const page = message.page;
    if (!Number.isInteger(page) || page < 1 || !pageCount) return;
    go({ page: Math.min(page, pageCount), y: null });
    return;
  }
  if (message.type === "viewer-sidebar") {
    if (typeof message.open !== "boolean" || presenting) return;
    if (sidebarOpen === message.open) return report();
    sidebarOpen = message.open;
    applyChrome();
    report();
    redraw();
    return;
  }
  if (message.type === "viewer-spread") {
    if (typeof message.spread !== "boolean" || presenting) return;
    if (spread === message.spread) return report();
    spread = message.spread;
    forcedAnchor = null;
    report();
    redraw();
    return;
  }
  if (message.type === "viewer-invert") {
    if (typeof message.invert !== "boolean") return;
    invert = message.invert;
    document.documentElement.toggleAttribute("data-invert", invert);
    report();
    return;
  }
  if (message.type === "viewer-find") {
    const { query, step } = message;
    if (typeof query !== "string" || query.length > 200 || ![-1, 0, 1].includes(step)) return;
    findQuery = query;
    if (!query) {
      findState = null;
      find.clear();
      report();
    } else {
      find.search(query, step, currentPage || 1);
    }
    return;
  }
  if (message.type === "viewer-present") {
    if (typeof message.on !== "boolean" || message.on === presenting) return;
    if (message.on && !drawn) return;
    setPresenting(message.on);
    return;
  }
  if (message.type === "viewer-density") {
    const zoom = message.zoom;
    if (typeof zoom !== "number" || !Number.isFinite(zoom) || zoom < 0.5 || zoom > 3) return;
    if (zoom === density) return;
    density = zoom;
    sidebar.setDensity(zoom);
    redraw();
    return;
  }
  // A frame that has just loaded says nothing until it is asked: the header
  // sends this when it appears, so controls for a page drawn before the
  // reader last navigated come back in the right state.
  if (message.type === "reader-ready") {
    report(true);
    return;
  }
  if (message.type !== "preview" || !message.pdf) return;

  // The bytes cross the frame boundary as an ArrayBuffer -- structured clone
  // takes one whole, where a PDF re-encoded as a string would not survive the
  // trip. Anything else is a caller that has not read this file.
  const bytes = message.pdf;
  if (!(bytes instanceof ArrayBuffer) && !ArrayBuffer.isView(bytes)) return;
  forcedAnchor = null;
  await paint(bytes);
});

/// "auto", the fit modes, or a zoom between 0.1 and 10 as a number string.
function validMode(mode) {
  if (typeof mode !== "string") return false;
  if (["auto", "page-width", "page-fit", "page-actual"].includes(mode)) return true;
  const value = Number(mode);
  return mode.trim() !== "" && Number.isFinite(value) && value >= 0.1 && value <= 10;
}

waiting("Nothing to show yet.", "This document has not been rendered in this browser.");
