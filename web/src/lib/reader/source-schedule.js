// When a source change repaints the preview, and when the preview target is
// worked out again.
//
// `generation` counts source changes. Renders capture it and are dropped when
// it has moved on, so a paint that finishes after an edit never lands. The
// scheduling is the page's to hand in: the editor, the format, the LaTeX
// debounce, and the paint and retarget actions, so that the cadence can be
// tested without mounting Reader.svelte.

/**
 * @param {{
 *   editing?: () => boolean,
 *   format?: () => string,
 *   latexDebounce?: () => number,
 *   continuous?: () => boolean,
 *   paint?: () => unknown,
 *   retarget?: () => unknown,
 *   hidden?: () => boolean,
 *   requestFrame?: (run: () => void) => unknown,
 *   cancelFrame?: (id: unknown) => void,
 *   setTimer?: (run: () => void, ms: number) => unknown,
 *   clearTimer?: (id: unknown) => void,
 *   passive?: number,
 *   remoteDelay?: number,
 *   targetPause?: number,
 *   typingInterval?: number,
 *   now?: () => number,
 * }} options
 */
export function createSourceSchedule({
  // This browser is editing the source.
  editing = () => false,
  // The preview's source format.
  format = () => "",
  // `latex.debounce()`: how long a LaTeX compile waits for a quiet source.
  latexDebounce = () => 0,
  // True when LaTeX should compile while typing, on the fast formats' cadence,
  // instead of waiting for a quiet source.
  continuous = () => false,
  // Paints the preview.
  paint = () => {},
  // Works out the preview target again.
  retarget = () => {},
  // True when the page is hidden, where frames do not run.
  hidden = () => (typeof document !== "undefined" && document.hidden),
  requestFrame = globalThis.requestAnimationFrame,
  cancelFrame = globalThis.cancelAnimationFrame,
  setTimer = setTimeout,
  clearTimer = clearTimeout,
  // A source that is not actively being edited in this browser waits for a
  // pause, so remote changes and preview-only views do not render every word.
  passive = 1000,
  // A collaborator's edit while this browser is editing waits this long, not
  // for a frame, so the preview keeps up with a colleague without repainting
  // on every one of their keystrokes.
  remoteDelay = 1000,
  // Any edit can change which file includes which, so the preview target is
  // worked out again once typing pauses, so the texts are read once per pause
  // and not once per keystroke.
  targetPause = 500,
  // While this browser types, the preview repaints at most this often, so a
  // repaint does not follow every keystroke. The last keystroke still paints.
  typingInterval = 150,
  // The clock that times the gap between paints. It is injected so that tests
  // can move time by hand.
  now = () => (globalThis.performance?.now?.() ?? Date.now()),
} = {}) {
  let generation = 0;
  let previewTimer = null;
  let previewFrame = null;
  let targetTimer = null;
  // The timer that holds a typing repaint back until `typingInterval` has passed
  // since the last paint. It is kept apart from `previewTimer`, which has
  // remote-edit semantics of its own.
  let capTimer = null;
  // When the last paint ran, from `now()`. `null` until the first paint, so the
  // first keystroke after start-up paints on the next frame.
  let lastPaint = null;
  // Set when a paint was due while the page was hidden. The paint is held
  // back rather than run, because a hidden tab renders nothing; `resume()`
  // draws it once the tab is shown again.
  let stale = false;

  // Every paint goes through here, so a hidden page never paints and records
  // that it skipped one. The paint time is recorded only when it does paint,
  // so a skipped render does not count as the last one.
  function run() {
    if (hidden()) {
      stale = true;
      return;
    }
    lastPaint = now();
    paint();
  }

  // The frame callback: the frame is spent, and the paint runs.
  function paintFrame() {
    previewFrame = null;
    run();
  }

  // Fast formats paint on the next animation frame, so every keystroke in one
  // frame shares one request and the coordinator queues at most one more.
  // While this browser types, the frame is also capped to one per
  // `typingInterval` (see `changed()`). A repaint per keystroke cost several
  // milliseconds of main-thread time each, so the preview is repainted about
  // seven times a second during continuous typing and catches up as soon as
  // typing stops.
  // Frames do not run in a hidden tab, so a hidden page uses a zero timer.
  // Returns a function that cancels the pending paint.
  function requestPreviewFrame(paintLater) {
    if (typeof requestFrame === "function" && !hidden()) {
      const id = requestFrame(paintLater);
      return () => cancelFrame(id);
    }
    const id = setTimer(paintLater, 0);
    return () => clearTimer(id);
  }

  function changed({ remote = false } = {}) {
    // Fast formats use a short bounded preview cadence. Only LaTeX owns its
    // longer compiler debounce; postponing this timer for every Typst
    // keystroke would starve the preview indefinitely. While this browser
    // types, the cadence is capped to one paint per `typingInterval`: a
    // keystroke inside the interval waits out the rest of it on `capTimer`,
    // and a keystroke after a quiet spell still paints on the next frame.
    if (editing() && (format() !== "latex" || continuous())) {
      if (remote) {
        // A render is already coming. Leaving the timer alone, rather than
        // pushing it back, lets a collaborator typing without a pause still
        // show up about once per second. A capped local repaint counts as
        // coming too, because it renders the source as it is when it runs.
        if (previewFrame !== null || previewTimer !== null || capTimer !== null) return;
        previewTimer = setTimer(() => {
          previewTimer = null;
          run();
        }, remoteDelay);
        return;
      }
      // A remote timer left pending must not hold back this browser's own
      // edit, so it is cleared. A debounce timer left over from before
      // continuous mode began is finished with for the same reason.
      if (previewTimer !== null) {
        clearTimer(previewTimer);
        previewTimer = null;
      }
      // A repaint is already due. It renders the source as it is when it
      // runs, so this keystroke is painted by that repaint and needs none of
      // its own.
      if (previewFrame !== null || capTimer !== null) return;
      // The gap since the last paint decides whether this keystroke may paint
      // now. A burst within `typingInterval` of a paint is held back, so the
      // preview moves about seven times a second rather than per keystroke.
      const since = lastPaint === null ? typingInterval : now() - lastPaint;
      if (since >= typingInterval) {
        previewFrame = requestPreviewFrame(paintFrame);
        return;
      }
      // The cap timer asks for the frame once the interval has passed. The
      // frame reads the source when it runs, so the text painted is the last
      // keystroke's, even when the burst ended inside the interval.
      capTimer = setTimer(() => {
        capTimer = null;
        previewFrame = requestPreviewFrame(paintFrame);
      }, typingInterval - since);
      return;
    }
    if (previewFrame !== null) {
      previewFrame();
      previewFrame = null;
    }
    // A capped repaint still waiting is dropped with the frame, because this
    // page is no longer on the fast path that would have painted it.
    clearTimer(capTimer);
    capTimer = null;
    clearTimer(previewTimer);
    // A LaTeX compile takes seconds, so it waits for the source to be quiet
    // for longer -- `latex.debounce()`, which adapts to the last compile pass.
    // A reader watching somebody else type waits longer still, and the longer
    // of the two wins.
    const wait =
      format() === "latex"
        ? Math.max(latexDebounce(), editing() ? 0 : passive)
        : passive;
    // The timer is cleared on the way out, so a fired id is nulled here. A
    // stale id would otherwise make the remote check above wait on a timer
    // that is no longer pending.
    previewTimer = setTimer(() => {
      previewTimer = null;
      run();
    }, wait);
  }

  return {
    get generation() { return generation; },
    bump() { generation += 1; },
    changed,
    // Cancels the pending preview timer and the pending typing cap. A pending
    // frame is left alone.
    cancel() {
      clearTimer(previewTimer);
      previewTimer = null;
      clearTimer(capTimer);
      capTimer = null;
    },
    // Paints a render that a hidden page skipped. Called when the page is
    // shown again.
    resume() {
      if (!stale) return;
      stale = false;
      paint();
    },
    scheduleTarget() {
      clearTimer(targetTimer);
      targetTimer = setTimer(() => retarget(), targetPause);
    },
    dispose() {
      clearTimer(previewTimer);
      clearTimer(capTimer);
      clearTimer(targetTimer);
      previewTimer = null;
      capTimer = null;
      stale = false;
    },
  };
}
