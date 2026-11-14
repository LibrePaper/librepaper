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
} = {}) {
  let generation = 0;
  let previewTimer = null;
  let previewFrame = null;
  let targetTimer = null;
  // Set when a paint was due while the page was hidden. The paint is held
  // back rather than run, because a hidden tab renders nothing; `resume()`
  // draws it once the tab is shown again.
  let stale = false;

  // Every paint goes through here, so a hidden page never paints and records
  // that it skipped one.
  function run() {
    if (hidden()) {
      stale = true;
      return;
    }
    paint();
  }

  // Fast formats paint on the next animation frame, so every keystroke in one
  // frame shares one request and the coordinator queues at most one more.
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
    // keystroke would starve the preview indefinitely.
    if (editing() && (format() !== "latex" || continuous())) {
      if (remote) {
        // A render is already coming. Leaving the timer alone, rather than
        // pushing it back, lets a collaborator typing without a pause still
        // show up about once per second.
        if (previewFrame !== null || previewTimer !== null) return;
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
      if (previewFrame !== null) return;
      previewFrame = requestPreviewFrame(() => {
        previewFrame = null;
        run();
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
    // Cancels the pending preview timer. A pending frame is left alone.
    cancel() {
      clearTimer(previewTimer);
      previewTimer = null;
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
      clearTimer(targetTimer);
      previewTimer = null;
      stale = false;
    },
  };
}
