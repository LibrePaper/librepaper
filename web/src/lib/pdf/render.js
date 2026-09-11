// The PDF, drawn into the frame a document lives in.
//
// This is the rendered state of the LaTeX preview: every page's
// canvas and text layer, stacked vertically in one scrolling document, which
// is what makes the text of a LaTeX document one sequence rather than a pile
// of pages. pdf.js and its worker come from our own static assets and never
// from a CDN -- the documents origin's CSP allows `script-src 'self'` and a
// worker from `blob:`, and that is deliberate, not an oversight.
//
// It is loaded lazily by `viewer.js`, on the first PDF that arrives. pdf.js
// is the largest thing the web build has after the typst module, and a reader
// looking at a markdown document must never pay for it.

import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { piecesOf } from "./text.js";
import { viewerScale } from "./fit.js";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

/// Where each page starts in the joined text, so the caret lock and SyncTeX
/// -- steps 3 and 6, neither built here -- can ask "which page is this
/// offset on" without walking the DOM again. Kept as the cumulative length of
/// everything this module put into the body, which is the same number the
/// agent's own table arrives at because marks add no text.
let pageStarts = [];

export function pageForOffset(offset) {
  if (!pageStarts.length) return -1;
  let page = 0;
  while (page + 1 < pageStarts.length && pageStarts[page + 1] <= offset) page += 1;
  return page;
}

let generation = 0;

/// Open `bytes` as a pdf.js document, run `fn` against it, and tear the
/// worker down afterward. `fn` receives the resolved document and its
/// result becomes this function's result; whatever `fn` throws or returns
/// early propagates the same way, since the teardown lives in `finally` and
/// runs on every path.
async function withDocument(bytes, fn) {
  const loading = pdfjs.getDocument({
    data: bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes),
    // No fetching of anything, from anywhere, at draw time. A PDF that names
    // a standard font gets pdf.js's own metrics; one that names a URL gets
    // nothing, which is what a document served from a sandboxed origin
    // should get.
    isEvalSupported: false,
    disableFontFace: false,
  });
  let document_ = null;
  try {
    document_ = await loading.promise;
    return await fn(document_);
  } finally {
    // A loading task owns a worker even after its promise resolves. Destroy
    // both the document and task on success, failure, and generation
    // cancellation so repeated previews do not accumulate pdf.js workers.
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
}

/// Draw `bytes` into `root`, replacing whatever was there.
///
/// A second `preview` arrives on every recompile, so this has to be safe to
/// call again while the last one is still rendering: `generation` is the
/// token that lets a superseded run drop its pages on the floor instead of
/// appending them under the new document's.
export async function render(bytes, root, mode = "auto") {
  const mine = ++generation;
  return withDocument(bytes, async (document_) => {
    if (mine !== generation) return 0;

    // Everything is built off-document and swapped in at the end. Painting page
    // by page into the live body would have the agent's observer republish a
    // half-drawn document once per page, and the sidebar re-anchor every
    // comment against text that is about to grow.
    const staging = document.createElement("div");
    staging.className = "pages";
    const starts = [];
    let offset = 0;

    // Every page is drawn at one scale, chosen from the first page's width:
    // a document whose pages disagree about their size is still one column of
    // pages, and rescaling halfway down it would read as a mistake.
    const first = await document_.getPage(1);
    if (mine !== generation) return 0;
    const size = first.getViewport({ scale: 1 });
    const scale = viewerScale(mode, document.documentElement.clientWidth,
      document.documentElement.clientHeight - 40, size.width, size.height);
    staging.dataset.scale = String(scale / (96 / 72));
    first.cleanup();

    for (let number = 1; number <= document_.numPages; number++) {
      const page = await document_.getPage(number);
      if (mine !== generation) return 0;
      const viewport = page.getViewport({ scale });

      const frame = document.createElement("div");
      frame.className = "page";
      frame.dataset.page = String(number);
      frame.style.width = `${Math.floor(viewport.width)}px`;
      frame.style.height = `${Math.floor(viewport.height)}px`;
      // The text layer's spans size themselves from these, which is how the
      // selection overlay stays on top of the glyphs at any scale.
      frame.style.setProperty("--scale-factor", String(scale));
      frame.style.setProperty("--user-unit", "1");
      frame.style.setProperty("--total-scale-factor", String(scale));
      frame.style.setProperty("--scale-round-x", "1px");
      frame.style.setProperty("--scale-round-y", "1px");

      const ratio = Math.min(globalThis.devicePixelRatio || 1, 2);
      const canvas = document.createElement("canvas");
      canvas.width = Math.floor(viewport.width * ratio);
      canvas.height = Math.floor(viewport.height * ratio);
      canvas.style.width = `${Math.floor(viewport.width)}px`;
      canvas.style.height = `${Math.floor(viewport.height)}px`;
      frame.append(canvas);

      const layer = document.createElement("div");
      layer.className = "textLayer";
      frame.append(layer);
      staging.append(frame);

      await page.render({
        canvasContext: canvas.getContext("2d"),
        viewport,
        transform: ratio === 1 ? null : [ratio, 0, 0, ratio, 0, 0],
      }).promise;
      if (mine !== generation) return 0;

      const content = await page.getTextContent();
      const text = new pdfjs.TextLayer({ textContentSource: content, container: layer, viewport });
      await text.render();
      if (mine !== generation) return 0;

      starts.push(offset);
      rewrite(text.textDivs, content.items, number > 1);
      // pdf.js may omit a content item from `textDivs` (for example a font
      // run with no selectable glyphs). The agent anchors against the DOM it
      // can actually walk, so page offsets must use that DOM length rather
      // than the theoretical content-item total returned by rewrite().
      offset += frame.textContent.length;
      page.cleanup();
    }

    root.replaceChildren(staging);
    pageStarts = starts;
    return document_.numPages;
  });
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

export { createToolbar } from "./toolbar.js";
