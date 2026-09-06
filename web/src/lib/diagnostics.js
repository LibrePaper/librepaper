// When a diagnostic is painted.
//
// Half of what a compiler calls an error is a construct that is not finished
// being typed, so diagnostics are painted with an asymmetry: from clean
// to red at reading speed, from red to clean at typing speed. A diagnostic is
// painted only once the source has been quiet for `delay`, **measured from the
// keystroke** rather than from the render that noticed it -- a render takes as
// long as it takes, and a wait measured from the end of one is a wait of
// unknown length.
//
// It lives here rather than in the component so that the rule can be tested
// against a clock the test controls, which is what `checks/diagnostics.mjs`
// does.

export const DIAGNOSTIC_DELAY = 400;

/// `paint(list)` is called with what should be on the screen. `now`,
/// `setTimer` and `clearTimer` are injectable so a test can run a session in
/// no time at all.
export function painter({
  delay = DIAGNOSTIC_DELAY,
  paint,
  now = () => Date.now(),
  setTimer = setTimeout,
  clearTimer = clearTimeout,
} = {}) {
  let quietSince = now();
  let timer = null;

  function schedule(list) {
    clearTimer(timer);
    // From the keystroke, not from here. A render that took 300 ms of the
    // four hundred has already spent them.
    const remaining = Math.max(0, quietSince + delay - now());
    timer = setTimer(() => paint(list), remaining);
  }

  return {
    /// The source changed. Nothing is painted for it yet, and whatever was
    /// about to be painted for the state before it is no longer about to be:
    /// it described text that no longer exists.
    typed() {
      quietSince = now();
      clearTimer(timer);
      timer = null;
    },

    /// A render finished. `page` is whether it produced one.
    rendered({ page, diagnostics = [] }) {
      if (page) {
        // A successful render clears every diagnostic the moment it lands,
        // whether or not the delay has passed: that is the fast half of the
        // asymmetry. Its own warnings are painted on the slow schedule, so a
        // font name half typed does not flash a badge on every keystroke.
        clearTimer(timer);
        timer = null;
        paint([]);
        const warnings = diagnostics.filter((one) => one.severity === "warning");
        if (warnings.length) schedule(warnings);
        return;
      }
      schedule(diagnostics);
    },

    /// Nothing more is coming; drop any pending paint.
    cancel() {
      clearTimer(timer);
      timer = null;
    },
  };
}
