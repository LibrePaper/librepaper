// How the window is divided, and how wide each part of it may be.
//
// There are three arrangements and one of them is showing: the source and the
// document side by side, the source alone, or the document alone. Every one of
// them shows something, so there is no arrangement to refuse and no last pane
// to protect. The column is not part of that: it holds the files, the
// comments or the history, and it is either there or not, in reading and
// editing alike. It sits at the left of the window, before whatever the
// arrangement shows.
//
// The two kinds of pane are measured in two different units, because they are
// two different kinds of thing. The source and the document share what is left
// of the window, so their split is a fraction: half stays half when the window
// is resized or the column opens, which a pixel width cannot do. A comment
// card wants a readable width whatever the screen is, so the column is
// pixels. Both are remembered per reader, not per document.

import { read, write } from "./storage.js";

// What the document keeps beside a source dragged as wide as it will go.
export const DOCUMENT_MIN = 360;

// The width of a separator, which is room neither pane has.
export const GRIP = 6;

// The activity icons remain available when the sidebar content is closed.
export const ACTIVITY_WIDTH = 48;

export const LAYOUTS = ["split", "source", "document"];

// The ratios a drag sticks to, and the ones the menu offers by name. The two
// are the same list on purpose: what the pointer finds and what the menu says
// should not be different places.
export const RATIOS = [
  { share: 1 / 3, says: "Wide document" },
  { share: 1 / 2, says: "Even" },
  { share: 2 / 3, says: "Wide source" },
];
// How close to a ratio the pointer has to be to be taken there.
const MAGNET = 12;

export const PANES = {
  editor: {
    name: "editor",
    key: "komodoc-source-share",
    // A share of what the source and the document have between them.
    fraction: true,
    reset: 1 / 2,
    min: 320, // px, about the narrowest a line of source reads at
    max: 0.7,
  },
  sidebar: {
    name: "sidebar",
    key: "komodoc-sidebar",
    fraction: false,
    reset: 360, // px, a comfortable comment card
    min: 240, // px, about the narrowest a comment card reads at
    max: 0.6, // of the window, so what it sits beside always keeps 40%
  },
};

export const stored = (pane) => Number(read(pane.key, 0)) || pane.reset;
export const remember = (pane, size) => write(pane.key, size);

/// What is on the screen, given the arrangement and whether the column is
/// open. Editing decides whether there is a source at all: a document nobody
/// can edit has one arrangement, and it is the document. `comments` is the
/// column, whichever panel is in it: the layout only needs to know that the
/// column is there, not what it holds.
export function showing({ layout, comments, editing }) {
  return {
    source: editing && (layout === "split" || layout === "source"),
    document: !editing || layout === "split" || layout === "document",
    comments,
  };
}

/// The room the separators take, which is neither pane's to use.
export function separators(state) {
  const shown = showing(state);
  return (shown.source && shown.document ? GRIP : 0) + (shown.comments ? GRIP : 0);
}

/// What the source and the document have between them: the window, less the
/// column and every separator in it.
export function surface(state) {
  const shown = showing(state);
  return state.width - separators(state) - (shown.comments ? clamp(PANES.sidebar, state) : ACTIVITY_WIDTH);
}

/// The column, in pixels, within what the rest of the window can
/// spare -- which is what is left after the source and the document have the
/// least each of them can be read at. A window too narrow for all three used
/// to leave the column its full width and crush the document to a strip;
/// the column is the pane that gives way, down to its own minimum.
function clampSidebar(width, state) {
  const shown = showing(state);
  const spare =
    state.width -
    separators(state) -
    (shown.source ? PANES.editor.min : 0) -
    (shown.document ? DOCUMENT_MIN : 0);
  const ceiling = Math.max(PANES.sidebar.min, Math.min(state.width * PANES.sidebar.max, spare));
  return Math.round(Math.max(PANES.sidebar.min, Math.min(width, ceiling)));
}

/// The source's share of the surface, within what both of them can read at.
function clampShare(share, state) {
  const room = surface(state);
  if (room <= 0) return share;
  const floor = PANES.editor.min / room;
  const ceiling = Math.min(PANES.editor.max, (room - DOCUMENT_MIN) / room);
  // On a surface too narrow for both of them the ceiling is under the floor,
  // and the source is what gives: this is a place for reading a document, and
  // the source has an arrangement of its own one click away.
  return Math.min(Math.max(Math.min(share, ceiling), floor), Math.max(ceiling, 0));
}

/// A pane's size, clamped, in whatever unit that pane is kept in. `state.sizes`
/// holds what the reader last chose; passing a size asks about that one instead.
export function clamp(pane, state, size = state.sizes[pane.key]) {
  return pane.fraction ? clampShare(size, state) : clampSidebar(size, state);
}

/// And in pixels, which is what the stylesheet is given.
export function pixels(pane, state) {
  return pane.fraction
    ? Math.round(clamp(pane, state) * surface(state))
    : clamp(pane, state);
}

/// A pointer at x, as a size for this pane, in the pane's own unit. Which edge
/// it is measured from depends on where the pane sits: the column is always
/// first, and the source is on whichever side the reader put it.
export function sizeAt(pane, x, state) {
  if (!pane.fraction) return x;
  const room = surface(state);
  const width = state.sourceSide === "left" ? x - leftEdge(state) : state.width - x;
  return snap(width / room, room);
}

// Near one of the named ratios, take it: a split reachable in one drag without
// having to find it by hand.
function snap(share, room) {
  for (const ratio of RATIOS) {
    if (Math.abs(share - ratio.share) * room <= MAGNET) return ratio.share;
  }
  return share;
}

/// Whether a size is sitting on one of the named ratios, so the guide line can
/// say that it is.
export function snapped(pane, size) {
  return pane.fraction && RATIOS.some((ratio) => ratio.share === size);
}

/// Where this pane's separator sits, for the line that follows the pointer
/// while it is dragged.
export function edgeAt(pane, size, state) {
  if (!pane.fraction) return clampSidebar(size, state);
  const width = clampShare(size, state) * surface(state);
  return state.sourceSide === "left" ? leftEdge(state) + width : state.width - width;
}

/// A step of an arrow key, in the pane's own unit: about the same distance on
/// the screen either way.
export function step(pane, state, forward) {
  const by = pane.fraction ? 24 / Math.max(surface(state), 1) : 24;
  return clamp(pane, state, state.sizes[pane.key] + (forward ? by : -by));
}

/// Which arrow key makes this pane wider, since a separator is focusable and
/// should move without a pointer.
export function grows(pane, state) {
  if (!pane.fraction) return "ArrowRight";
  return state.sourceSide === "left" ? "ArrowRight" : "ArrowLeft";
}

// Where the source and the document begin: after the column and the separator
// beside it, when the column is open.
function leftEdge(state) {
  const shown = showing(state);
  return shown.comments ? clamp(PANES.sidebar, state) + GRIP : ACTIVITY_WIDTH;
}
