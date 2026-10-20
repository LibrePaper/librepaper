// How large a PDF page is drawn, given the room the frame has.
//
// Its own module, with no pdf.js in it, so `tests/unit/pdf-fit.mjs` can ask it
// the question without a browser or a corpus PDF. `render.js` is the only
// caller; `viewer.js` is what gives it a second chance after a resize.

// One and a half CSS pixels per PDF point is the scale a page is drawn at
// when there is room for it, doubled on a retina screen by the canvas'
// backing store rather than by the layout, so the text layer's percentages
// and the canvas agree at any device pixel ratio.
//
// There is not always room. The pane beside the source is half a window wide
// and a phone is narrower than one page at any magnification, so a fixed
// scale leaves a letter-sized page clipped on both sides with a horizontal
// scrollbar under it -- a document you scroll sideways to read a line of.
export const SCALE = 1.5;
// Below this the glyphs stop being readable, and scrolling sideways is the
// better bargain after all.
export const MIN_SCALE = 0.35;
// What `viewer.html` puts either side of the page: `main`'s 16px padding.
export const GUTTER = 32;

/// The scale `width` CSS pixels of frame can afford for a page `points` wide.
///
/// A width or a page it cannot measure gets `SCALE`: an unknown width is not
/// evidence that the frame is narrow, and drawing small on a wide screen is
/// the worse of the two mistakes.
export function scaleFor(width, points) {
  if (!(width > 0) || !(points > 0)) return SCALE;
  return Math.max(MIN_SCALE, Math.min(SCALE, (width - GUTTER) / points));
}

// The space `viewer.html` leaves between two pages, in a column or in a spread.
export const GAP = 24;

// PDF.js percentages use CSS pixels (96 dpi); PDF dimensions use 72 dpi.
//
// `pageWidth` and `pageHeight` are those of the widest and the tallest page,
// so no page of a mixed document overflows. `layout` describes the room:
// `columns` is 2 for a spread, where the width to fit is two pages and the
// gap between them; `sidebar` is the width the open sidebar takes off the
// frame; `bleed` is presentation, where the page may touch the edges of the
// screen and nothing is reserved for padding.
export function viewerScale(mode, width, height, pageWidth, pageHeight, layout = {}) {
  const { columns = 1, sidebar = 0, bleed = false } = layout;
  const actual = 96 / 72;
  const room = Math.max(1, width - sidebar - (bleed ? 0 : GUTTER) - GAP * (columns - 1));
  const fitWidth = room / (pageWidth * columns);
  const fitHeight = Math.max(1, height - (bleed ? 0 : 48)) / pageHeight;
  if (mode === "page-width") return fitWidth;
  if (mode === "page-fit") return Math.min(fitWidth, fitHeight);
  if (mode === "page-actual") return actual;
  if (mode === "auto") return Math.min(1.25 * actual, fitWidth);
  return Math.max(0.1, Math.min(10, Number(mode) || 1)) * actual;
}
