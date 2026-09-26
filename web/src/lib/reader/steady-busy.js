// A busy flag at reading speed.
//
// The preview's "Compiling…" indicator is a spinner and a word that mount
// into the header row and unmount again with the flag behind them. A render
// that takes 40ms is honestly reported by a flag that is true for 40ms, and
// the result is a control that appears and disappears several times a second
// while somebody types -- a strobe rather than a status, and one that moves
// the rest of the row each time. Typst and Markdown render that fast; LaTeX
// takes seconds and is what the indicator is actually for.
//
// So the flag is held to two numbers: nothing is shown until a render has
// been running long enough to be worth saying anything about, and anything
// shown stays up long enough to be read. Both are deliberately about people
// rather than about compilers, which is why they live here and not beside
// either renderer.
//
// The timers are arguments so a check can drive this without waiting.

/// How long a render must run before it is worth reporting.
export const APPEAR_AFTER_MS = 250;
/// How long a report stays up once it has been made.
export const HOLD_FOR_MS = 600;

/// Smooths `busy` for display. `onchange` is called with the shown value
/// whenever it changes, which is what the page renders from; `set` takes the
/// true value as often as it likes.
export function createSteadyBusy({
  appearAfter = APPEAR_AFTER_MS,
  holdFor = HOLD_FOR_MS,
  onchange = () => {},
  setTimer = globalThis.setTimeout,
  clearTimer = globalThis.clearTimeout,
  now = () => Date.now(),
} = {}) {
  let busy = false;
  let shown = false;
  let shownAt = 0;
  let timer = null;

  function clear() {
    if (timer !== null) clearTimer(timer);
    timer = null;
  }

  function show() {
    clear();
    if (shown) return;
    shown = true;
    shownAt = now();
    onchange(true);
  }

  function hide() {
    clear();
    if (!shown) return;
    shown = false;
    onchange(false);
  }

  /// The true flag, as often as it changes.
  function set(next) {
    next = Boolean(next);
    if (next === busy) return;
    busy = next;
    if (busy) {
      // Already up from the render before this one: keep it up rather than
      // taking it down and putting it back.
      if (shown) { clear(); return; }
      clear();
      timer = setTimer(show, appearAfter);
      return;
    }
    // Nothing is running any more. A report that was never made is abandoned;
    // one that was made stays for the rest of its hold.
    if (!shown) { clear(); return; }
    const remaining = Math.max(0, holdFor - (now() - shownAt));
    clear();
    if (!remaining) { hide(); return; }
    timer = setTimer(hide, remaining);
  }

  /// Everything pending is dropped and the flag goes down at once. For a
  /// reader leaving the document, where a held indicator has nothing left to
  /// describe.
  function stop() {
    clear();
    busy = false;
    if (shown) { shown = false; onchange(false); }
  }

  return { set, stop, get shown() { return shown; }, get busy() { return busy; } };
}
