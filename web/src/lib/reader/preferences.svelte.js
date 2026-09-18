// What this reader has arranged, and what they find when they come back.
//
// The arrangement of the window, which side the source is on, which keys the
// editor answers to, how wide the panes are: none of it is about the
// document, all of it is about the person at this browser, and all of it is
// remembered. The panel the column shows is the exception -- see below.
//
// It is here rather than in the page because remembering was the part that
// kept going wrong. Setting the arrangement and writing it down were two
// statements, repeated at eight call sites, and the second was easy to leave
// out -- an arrangement that lasted until the tab was closed and then
// quietly wasn't. Every setter below does both, so there is no way to change
// one of these and not remember it.
//
// Reading is the other half: a value out of storage is whatever was there,
// including a value from a version of the application that offered a choice
// this one does not, so every one of them is checked against what is on
// offer now rather than trusted.

import { KEYMAP, LAYOUT, SOURCE_SIDE, read, write } from "../storage.js";
import { LAYOUTS, PANES, remember, stored } from "../panes.js";

const MOBILE_VIEW = "librepaper-mobile-view";
const KEYMAPS = ["vim", "emacs"];
const MOBILE_VIEWS = ["document", "source", "sidebar"];

const oneOf = (options, value, fallback) => (options.includes(value) ? value : fallback);

export function createPreferences() {
  const state = $state({
    layout: oneOf(LAYOUTS, read(LAYOUT, "split"), "split"),
    sourceSide: read(SOURCE_SIDE, "left") === "right" ? "right" : "left",
    keys: oneOf(KEYMAPS, read(KEYMAP, "default"), "default"),
    // The one arrangement that is not remembered. Opening a document is
    // opening its files: whichever panel the last document was left on --
    // its history, its agent, its comments -- was about that document rather
    // than this one, and starting there means starting somewhere the reader
    // has to leave before they can see what they opened.
    panel: "files",
    collaborationTab: "comments",
    // Narrow screens show one workspace view at a time. Independent of the
    // desktop split, so widening the window restores the reader's layout.
    mobileView: oneOf(MOBILE_VIEWS, read(MOBILE_VIEW, "document"), "document"),
    preferredPane: read(LAYOUT, "split") === "source" ? "source" : "document",
    // The source and the document are kept as a share of what they have
    // between them; the comment column is kept in pixels. Two units because
    // they are two different kinds of pane: half a window stays half when the
    // window changes, and a comment card wants the same readable width
    // whatever the screen is.
    sizes: {
      [PANES.editor.key]: stored(PANES.editor),
      [PANES.sidebar.key]: stored(PANES.sidebar),
    },
  });

  return {
    state,
    setLayout(value) {
      state.layout = value;
      write(LAYOUT, value);
    },
    setSourceSide(side) {
      state.sourceSide = side;
      write(SOURCE_SIDE, side);
    },
    setKeys(next) {
      state.keys = next;
      write(KEYMAP, next);
    },
    /// The panel the column is showing, for this visit only. Nothing is
    /// written down: the next document opens on its files.
    setPanel(name) {
      state.panel = name;
    },
    setMobileView(view) {
      state.mobileView = view;
      write(MOBILE_VIEW, view);
    },
    /// What the reader asked a pane to be. What fits is worked out again
    /// every time it is needed: writing the fitted size back would make a
    /// narrow window permanent -- drag the window in and the split is
    /// squeezed, drag it out and it stays squeezed, because what was asked
    /// for is gone.
    setSize(pane, size) {
      state.sizes[pane.key] = size;
      remember(pane, size);
    },
  };
}
