// What the preview frame shows right now, and what it has shown since it last
// loaded.
//
// These facts used to be separate variables in Reader, each reset by hand in
// six places. They change together: a navigation, a clear, a new target and an
// empty document each reset the same group and differ only in what they keep.
// So they live here, with one `reset` that every such site calls.
//
// The reactive part is `state`. `ever` and `synctex` are plain on purpose and
// are not part of it.

/**
 * @typedef {{ shown: boolean, drawn: boolean, empty: boolean }} PaintedState
 */

/**
 * @returns {{
 *   state: PaintedState,
 *   readonly ever: boolean,
 *   synctex: any,
 *   reset: (options?: { keepSynctex?: boolean, empty?: boolean }) => void,
 *   delivered: () => void,
 *   drawn: () => void,
 *   navigated: () => void,
 * }}
 */
export function createPainted() {
  // `shown`: the frame has shown a page for the current target. Markup reads
  // this in place of `ever`, which it cannot read reactively.
  // `drawn`: the page for the current target is on the screen. For a PDF that
  // is the viewer saying it drew; a page that is only delivered is still blank
  // in a viewer that is loading. Reset wherever `shown` is.
  // `empty`: the render found nothing to draw: the file is known and has no
  // text. This settles the preview, so nothing is left to wait for. Reset
  // wherever `shown` is, and by any change to the source.
  const state = $state({ shown: false, drawn: false, empty: false });

  // Whether the frame has ever shown a page. Until it has, a document that does
  // not compile has nothing to keep on the screen. This is a plain variable on
  // purpose: it is written from inside a render and read nowhere near one. The
  // reader's "not yet rendered" is the one place the question is asked from the
  // template, and that reads `state.shown`, the same fact in a reactive form.
  let ever = false;

  // The SyncTeX click map of the last render. It is read from the source and
  // from clicks in the preview, never from markup, so it is not reactive.
  let synctex = null;

  return {
    state,
    get ever() {
      return ever;
    },
    get synctex() {
      return synctex;
    },
    set synctex(value) {
      synctex = value;
    },
    /// Forget what the frame shows. `keepSynctex` keeps the click map, and
    /// `empty` says the render found nothing to draw rather than that nothing
    /// has been drawn yet.
    reset({ keepSynctex = false, empty = false } = {}) {
      ever = false;
      state.shown = false;
      state.drawn = false;
      state.empty = empty;
      if (!keepSynctex) synctex = null;
    },
    /// The frame was handed a payload, and so shows a page now.
    delivered() {
      ever = true;
      state.shown = true;
      state.empty = false;
    },
    /// The viewer says the page for the current target is drawn.
    drawn() {
      state.drawn = true;
    },
    /// A new frame has nothing drawn in it and no click map for it.
    navigated() {
      state.drawn = false;
      synctex = null;
    },
  };
}
