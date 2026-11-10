// What the workspace is arranged as: which panes and which panel are on
// screen, on a desktop and on a phone.
//
// The reactive part is `state`: the layout the reader chose, the panel in the
// column, the phone's view and overlays, and the width and rem the panes are
// measured against. What those add up to -- whether the window is a phone,
// whether the source is open, which face the phone shows, what is shown -- is
// derived here and read through the getters. The things the arrangement needs
// from the page (whether the reader is editing, whether a comparison is up,
// which panel the column falls back to) come in as lazy callbacks, so that
// the owner can be made before they exist and driven without a page.

import { ACTIVITY_WIDTH, DOCUMENT_MIN, GRIP, PANES, measure, px, showing } from "../panes.js";

/**
 * @typedef {{
 *   layout: string,
 *   panel: string,
 *   mobileView: string,
 *   preferredPane: string,
 *   moreOpen: boolean,
 *   mobileCommand: string,
 *   keyboardOpen: boolean,
 *   width: number,
 *   unit: number,
 * }} ArrangementState
 */

/**
 * @param {{
 *   editing?: () => boolean,
 *   comparing?: () => string,
 *   sourceSide?: () => string,
 *   sizes?: () => Record<string, number>,
 *   home?: () => string,
 *   blocked?: (name: string) => boolean,
 *   leaving?: (name: string) => void,
 *   loadHistory?: () => Promise<unknown>,
 *   initialWidth?: () => number,
 *   measureUnit?: () => number,
 * }} options
 */
export function createArrangement({
  // Whether the reader is editing, which is what opens the source.
  editing = () => false,
  // The kind of comparison showing in place of the source, or "".
  comparing = () => "",
  // Which side the source is on.
  sourceSide = () => "left",
  // How wide the panes are.
  sizes = () => ({}),
  // The panel the column goes back to when what it showed is taken away.
  home = () => "",
  // True for a panel this build does not offer.
  blocked = (_name) => false,
  // Called with the panel about to be shown, before it is: leaving the panel a
  // comparison belongs to ends the comparison.
  leaving = (_name) => {},
  // Fetches the history manifest.
  loadHistory = () => Promise.resolve(),
  // The width the panes are first divided out of.
  initialWidth = () => (typeof innerWidth === "number" ? innerWidth : 1280),
  // What a rem is worth, in pixels.
  measureUnit = measure,
} = {}) {
  // Every project opens the same way: the files, the file it opens on and its
  // preview, or the preview alone on a phone. Where the last visit ended is
  // not kept.
  //
  // The phone's overlays. The More sheet, and one of the command lists
  // ("file", "edit", "view", "insert"), each open over the document; the
  // keyboard is whether the on-screen keyboard is up. A panel is an overlay
  // too, but it is `mobileView === "sidebar"` and is kept as it always was.
  //
  // `width` is the width the panes are divided out of. Bound rather than read
  // when something happens to ask: it is what every size below is measured
  // against, and a measurement taken once is a layout that is right until the
  // window moves.
  // And `unit` is what a rem is worth, since every limit in panes.js is one. It
  // moves when the reader changes their text size or zooms, which is a resize;
  // it is held here rather than read where it is used so that what depends on
  // it is worked out again when it does.
  const state = $state({
    layout: "split",
    panel: "files",
    mobileView: "document",
    preferredPane: "document",
    moreOpen: false,
    mobileCommand: "",
    keyboardOpen: false,
    width: initialWidth(),
    unit: measureUnit(),
  });

  const compact = $derived(state.width < 768);
  // The source pane is open when the reader is editing, or a comparison is
  // showing in its place.
  const sourceOpen = $derived(editing() || comparing() !== "");
  const activeMobileView = $derived(comparing() === "history" && state.mobileView === "document" ? "source"
    : state.mobileView === "source" && !sourceOpen ? "document"
    : state.mobileView === "sidebar" && !state.panel ? "document" : state.mobileView);
  const splitTight = $derived(state.width < px(ACTIVITY_WIDTH, state.unit) + px(PANES.editor.min, state.unit)
    + px(DOCUMENT_MIN, state.unit) + px(GRIP, state.unit)
    + (state.panel ? px(PANES.sidebar.min, state.unit) + px(GRIP, state.unit) : 0));
  // A window that cannot hold the split it was asked for: the phone, and the
  // tight split above it. Both show one pane at a time, so both need the
  // switch between the two faces in the bar; only the phone loses the rail
  // beside the column, and reaches the panels from the Menu in the top bar.
  const adapted = $derived(compact || (splitTight && state.layout === "split"));
  const effectiveLayout = $derived(!compact && comparing() === "history"
    ? "source"
    : compact
    ? (activeMobileView === "source" ? "source" : "document")
    : state.layout === "split" && splitTight ? state.preferredPane : state.layout);

  // What every measurement below is made against.
  // The column holds one of three things -- the files, the comments or the
  // timeline -- so what the layout needs to know is whether it is there, not
  // which of them is in it.
  const panes = $derived({
    layout: effectiveLayout,
    comments: Boolean(state.panel),
    editing: sourceOpen,
    sourceSide: sourceSide(),
    sizes: sizes(),
    width: state.width,
    // Passed so that a change of text size reaches everything measured in rem,
    // which is every limit there is: `px` holds the measurement itself, and
    // what is never seen to read it is never worked out again.
    unit: state.unit,
  });
  const shown = $derived(compact ? {
    source: sourceOpen && activeMobileView === "source",
    document: activeMobileView === "document",
    comments: activeMobileView === "sidebar",
  } : showing(panes));

  function showMobileView(view) {
    if (view === "source" && !sourceOpen) view = "document";
    state.mobileView = view;
    if (view !== "sidebar") state.preferredPane = view;
    if (!compact && view !== "sidebar" && state.layout !== "split") state.layout = view;
    if (view === "sidebar" && !state.panel) showPanel(home());
  }
  function selectPanel(name) {
    if (blocked(name)) return;
    const next = state.panel === name && (!compact || activeMobileView === "sidebar") ? "" : name;
    showPanel(next);
    if (compact) showMobileView(next ? "sidebar" : "document");
  }
  // The files panel, brought forward wherever it is drawn.
  function openFilesPanel() {
    void showPanel("files");
    if (compact) showMobileView("sidebar");
  }

  // Showing a panel; "" closes the column. Opening the timeline is what
  // fetches the manifest; leaving it drops the comparison, which is all the
  // panel ever held -- the document pane shows the current document
  // throughout.
  function showPanel(name) {
    if (blocked(name)) return Promise.resolve();
    // Leaving the panel a comparison belongs to ends the comparison.
    leaving(name);
    state.panel = name;
    if (compact) state.mobileView = name ? "sidebar" : state.preferredPane;
    if (name !== "history") return Promise.resolve();
    return loadHistory();
  }

  return {
    state,
    get compact() { return compact; },
    get sourceOpen() { return sourceOpen; },
    get activeMobileView() { return activeMobileView; },
    get splitTight() { return splitTight; },
    get adapted() { return adapted; },
    get effectiveLayout() { return effectiveLayout; },
    get panes() { return panes; },
    get shown() { return shown; },
    showMobileView,
    selectPanel,
    openFilesPanel,
    showPanel,
  };
}
