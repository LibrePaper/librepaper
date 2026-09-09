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

let viewer = null; // the render module, once something has needed it
let stage = null;
let toolbar = null;
let scaleMode = "auto";
let paintGeneration = 0;
// The last PDF drawn, kept so a resize can redraw it. The pages are sized to
// the width the frame has (see `pdf/render.js`), and the frame's width is not
// fixed: the pane beside the source is dragged, the layout button gives the
// document the whole window, and a phone is turned on its side. Without this
// the page keeps whatever width it was first drawn at and is clipped or
// stranded in the middle of the frame.
let drawn = null;

function ready() {
  if (stage) return stage;
  document.body.replaceChildren();
  stage = document.createElement("main");
  toolbar = viewer.createToolbar((mode) => {
    scaleMode = mode;
    if (drawn) void paint(drawn);
  });
  document.body.append(toolbar.element, stage);
  return stage;
}

function waiting(message) {
  const note = document.createElement("p");
  note.className = "note";
  note.textContent = message;
  toolbar?.destroy();
  toolbar = null;
  document.body.replaceChildren(note);
  stage = null;
  drawn = null;
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
  try {
    viewer ??= await import("../lib/pdf/render.js");
    const pages = await viewer.render(keep.slice(), ready(), scaleMode);
    if (mine !== paintGeneration) return;
    toolbar.update(scaleMode, Number(stage.firstElementChild.dataset.scale));
    // The page index for an offset, for the caret lock and SyncTeX. Neither
    // is built here; both need this and nothing else from the viewer, so it
    // is exposed now rather than left for them to reach into the DOM for.
    globalThis.librepaperViewer = { pageForOffset: viewer.pageForOffset, pages };
    // The agent republishes off its own mutation observer, which the swap in
    // `render` has just tripped: one code path for "the document changed",
    // whether the change was an HTML preview or a page finishing. There is
    // deliberately no second signal here.
  } catch (error) {
    if (mine !== paintGeneration) return;
    waiting(`this PDF could not be drawn: ${error}`);
  }
}

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
    void paint(drawn);
  }, 150);
});

addEventListener("message", async (event) => {
  if (event.source !== parent) return;
  const message = event.data;
  if (!message || message.librepaper !== true) return;
  if (message.type !== "preview" || !message.pdf) return;

  // The bytes cross the frame boundary as an ArrayBuffer -- structured clone
  // takes one whole, where a PDF re-encoded as a string would not survive the
  // trip. Anything else is a caller that has not read this file.
  const bytes = message.pdf;
  if (!(bytes instanceof ArrayBuffer) && !ArrayBuffer.isView(bytes)) return;
  lastWidth = document.documentElement.clientWidth;
  await paint(bytes);
});

waiting("nothing to show yet");
