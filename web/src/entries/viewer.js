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

function ready() {
  if (stage) return stage;
  document.body.replaceChildren();
  stage = document.createElement("main");
  document.body.append(stage);
  return stage;
}

function waiting(message) {
  const note = document.createElement("p");
  note.className = "note";
  note.textContent = message;
  document.body.replaceChildren(note);
  stage = null;
}

addEventListener("message", async (event) => {
  if (event.source !== parent) return;
  const message = event.data;
  if (!message || message.komodoc !== true) return;
  if (message.type !== "preview" || !message.pdf) return;

  // The bytes cross the frame boundary as an ArrayBuffer -- structured clone
  // takes one whole, where a PDF re-encoded as a string would not survive the
  // trip. Anything else is a caller that has not read this file.
  const bytes = message.pdf;
  if (!(bytes instanceof ArrayBuffer) && !ArrayBuffer.isView(bytes)) return;

  try {
    viewer ??= await import("../lib/pdf/render.js");
    const pages = await viewer.render(bytes, ready());
    // The page index for an offset, for the caret lock and SyncTeX. Neither
    // is built here; both need this and nothing else from the viewer, so it
    // is exposed now rather than left for them to reach into the DOM for.
    globalThis.komodocViewer = { pageForOffset: viewer.pageForOffset, pages };
    // The agent republishes off its own mutation observer, which the swap in
    // `render` has just tripped: one code path for "the document changed",
    // whether the change was an HTML preview or a page finishing. There is
    // deliberately no second signal here.
  } catch (error) {
    waiting(`this PDF could not be drawn: ${error}`);
  }
});

waiting("nothing to show yet");
