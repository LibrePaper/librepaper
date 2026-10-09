// The habits this reader keeps between visits.
//
// Which side the source is on, which keys the editor answers to, how wide the
// panes are: none of it is about the document, all of it is about the person
// at this browser, and all of it is remembered. Where the last visit ended is
// not: every project opens on its files, its main file and its preview.
//
// It is here rather than in the page because remembering was the part that
// kept going wrong. Setting a habit and writing it down were two statements,
// and the second was easy to leave out. Every setter below does both, so
// there is no way to change one of these and not remember it.
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
    sourceSide: read(SOURCE_SIDE, "left") === "right" ? "right" : "left",
    keys: oneOf(KEYMAPS, read(KEYMAP, "default"), "default"),
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
    setSourceSide(side) {
      state.sourceSide = side;
      write(SOURCE_SIDE, side);
    },
    setKeys(next) {
      state.keys = next;
      write(KEYMAP, next);
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
