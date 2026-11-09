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
 *   paint?: () => unknown,
 *   retarget?: () => unknown,
 *   hidden?: () => boolean,
 *   requestFrame?: (run: () => void) => unknown,
 *   cancelFrame?: (id: unknown) => void,
 *   setTimer?: (run: () => void, ms: number) => unknown,
 *   clearTimer?: (id: unknown) => void,
 *   passive?: number,
 *   targetPause?: number,
 * }} options
 */
export function createSourceSchedule({
  // This browser is editing the source.
  editing = () => false,
  // The preview's source format.
  format = () => "",
  // `latex.debounce()`: how long a LaTeX compile waits for a quiet source.
  latexDebounce = () => 0,
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
  // Any edit can change which file includes which, so the preview target is
  // worked out again once typing pauses, so the texts are read once per pause
  // and not once per keystroke.
  targetPause = 500,
} = {}) {
  let generation = 0;
  let previewTimer = null;
  let previewFrame = null;
  let targetTimer = null;

  // Fast formats paint on the next animation frame, so every keystroke in one
  // frame shares one request and the coordinator queues at most one more.
  // Frames do not run in a hidden tab, so a hidden page uses a zero timer.
  // Returns a function that cancels the pending paint.
  function requestPreviewFrame(run) {
    if (typeof requestFrame === "function" && !hidden()) {
      const id = requestFrame(run);
      return () => cancelFrame(id);
    }
    const id = setTimer(run, 0);
    return () => clearTimer(id);
  }

  function changed() {
    // Fast formats use a short bounded preview cadence. Only LaTeX owns its
    // longer compiler debounce; postponing this timer for every Typst
    // keystroke would starve the preview indefinitely.
    if (editing() && format() !== "latex") {
      if (previewFrame !== null || previewTimer !== null) return;
      previewFrame = requestPreviewFrame(() => {
        previewFrame = null;
        paint();
      });
      return;
    }
    if (previewFrame !== null) {
      previewFrame();
      previewFrame = null;
    }
    clearTimer(previewTimer);
    // A LaTeX compile takes seconds, so it waits for the source to be quiet
    // for longer -- `latex.debounce()`, which adapts to the last compile pass.
    // A reader watching somebody else type waits longer still, and the longer
    // of the two wins.
    const wait =
      format() === "latex"
        ? Math.max(latexDebounce(), editing() ? 0 : passive)
        : passive;
    previewTimer = setTimer(paint, wait);
  }

  return {
    get generation() { return generation; },
    bump() { generation += 1; },
    changed,
    // Cancels the pending preview timer. A pending frame is left alone.
    cancel() {
      clearTimer(previewTimer);
      previewTimer = null;
    },
    scheduleTarget() {
      clearTimer(targetTimer);
      targetTimer = setTimer(() => retarget(), targetPause);
    },
    dispose() {
      clearTimer(previewTimer);
      clearTimer(targetTimer);
      previewTimer = null;
    },
  };
}
