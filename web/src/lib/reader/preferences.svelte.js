// How a project opens, and the habits this reader keeps between visits.
//
// Every project opens the same way: the files in the sidebar, the main file in
// the source, and that file's preview in the document pane. On a phone the
// project view opens first. None of that is remembered, so a project always
// lands on the same arrangement.
//
// What is remembered is the reader's own habits: which side the source is on,
// which keys the editor answers to, and how wide the panes are. None of that is
// about the document, and all of it is about the person at this browser.
//
// It is here rather than in the page because remembering was the part that
// kept going wrong. Setting a habit and writing it down were two statements,
// repeated at several call sites, and the second was easy to leave out. Every
// setter below that is a habit does both, so there is no way to change one and
// not remember it. The arrangement setters change state only.
//
// Reading is the other half: a value out of storage is whatever was there,
// including a value from a version of the application that offered a choice
// this one does not, so every one of them is checked against what is on
// offer now rather than trusted.

import { KEYMAP, SOURCE_SIDE, read, write } from "../storage.js";
import { PANES, remember, stored } from "../panes.js";

const KEYMAPS = ["vim", "emacs"];

const oneOf = (options, value, fallback) => (options.includes(value) ? value : fallback);

export function createPreferences() {
  const state = $state({
    // The arrangement a project opens in. It is not remembered.
    layout: "split",
    sourceSide: read(SOURCE_SIDE, "left") === "right" ? "right" : "left",
    keys: oneOf(KEYMAPS, read(KEYMAP, "default"), "default"),
    // The sidebar opens on the files of the project.
    panel: "files",
    collaborationTab: "comments",
    // Narrow screens show one workspace view at a time. Independent of the
    // desktop split, so widening the window restores the split.
    mobileView: "sidebar",
    preferredPane: "document",
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
    // The arrangement is set for this visit only. It is not written down.
    setLayout(value) {
      state.layout = value;
    },
    setSourceSide(side) {
      state.sourceSide = side;
      write(SOURCE_SIDE, side);
    },
    setKeys(next) {
      state.keys = next;
      write(KEYMAP, next);
    },
    // The panel the column is showing. It is not written down, so a project
    // opens on its files again.
    setPanel(name) {
      state.panel = name;
    },
    // The mobile view is set for this visit only. It is not written down.
    setMobileView(view) {
      state.mobileView = view;
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
