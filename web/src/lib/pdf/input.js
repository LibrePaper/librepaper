// Wheel, keyboard and gesture handling for the PDF frame.
//
// The frame is a plain scrolling document, so most input is the browser's own.
// This module claims only what the browser does badly or not at all: keeping a
// diagonal trackpad swipe on one axis, and the keys a PDF reader is expected
// to have. Ctrl+wheel, which is also what a trackpad pinch sends in Chromium
// and Firefox, is `watchPinch`'s alone (`lib/pinch.js`), so a zoom gesture is
// seen once; the wheel handler here lets it pass untouched. It never zooms or
// scrolls by itself: it reports what was asked for through the callbacks, and
// `viewer.js` decides.
//
// The decisions are pure functions, exported for `tests/unit/pdf-input.mjs`;
// `installInput` is the thin layer that reads the DOM and calls them.
//
// `c`, `C` and Escape are left alone: `frame.js` binds them.

/// Wheel deltas arrive in pixels, lines or pages depending on the device.
const LINE = 16;

/// Pixels for one wheel delta, whatever unit the event used.
export function normalizeDelta(delta, deltaMode, pageHeight) {
  if (deltaMode === 1) return delta * LINE;
  if (deltaMode === 2) return delta * pageHeight;
  return delta;
}

/// "x" or "y" when one delta is at least twice the other, else null: a
/// gesture that starts off the diagonal has not yet said which way it goes.
export function dominantAxis(dx, dy) {
  const ax = Math.abs(dx);
  const ay = Math.abs(dy);
  if (ay >= 2 * ax && ay > 0) return "y";
  if (ax >= 2 * ay && ax > 0) return "x";
  return null;
}

/// How long, in ms, a pause ends a wheel gesture. Inertial scrolling keeps
/// events under this apart; a new flick comes after more.
export const GESTURE_GAP = 150;

/// The axis lock for plain wheel gestures. `next(time, dx, dy)` returns the
/// locked axis after an event, or null while it is still undecided.
export function createAxisLock() {
  let axis = null;
  let last = -Infinity;
  return {
    next(time, dx, dy) {
      if (time - last > GESTURE_GAP) axis = null;
      last = time;
      if (axis === null) axis = dominantAxis(dx, dy);
      return axis;
    },
  };
}

/// What a key press asks for, as `{ kind, arg }`, or null for a key that is
/// not ours. `key` is the event's `key`; `overflow` says whether the page
/// scrolls sideways, in which case the arrows are the browser's.
export function keyAction({ key, ctrl, meta, shift, alt }, presenting, overflow) {
  if (alt) return null;
  if (ctrl || meta) {
    if (key === "=" || key === "+") return { kind: "zoomStep", arg: 1.1 };
    if (key === "-" || key === "_") return { kind: "zoomStep", arg: 1 / 1.1 };
    if (key === "0") return { kind: "zoomReset" };
    if (key === "f" || key === "F") return { kind: "find" };
    return null;
  }
  if (presenting) {
    if (shift && key === " ") return { kind: "pageStep", arg: -1 };
    if (shift) return null;
    if (["ArrowDown", "ArrowRight", "PageDown", " "].includes(key)) return { kind: "pageStep", arg: 1 };
    if (["ArrowUp", "ArrowLeft", "PageUp", "Backspace"].includes(key)) return { kind: "pageStep", arg: -1 };
  }
  if (shift) return null;
  if (key === "Home") return { kind: "pageEdge", arg: "first" };
  if (key === "End") return { kind: "pageEdge", arg: "last" };
  if (!overflow && key === "ArrowLeft") return { kind: "pageStep", arg: -1 };
  if (!overflow && key === "ArrowRight") return { kind: "pageStep", arg: 1 };
  return null;
}

// A key typed into a field is the field's.
function typing(target) {
  if (!(target instanceof Element)) return false;
  return Boolean(target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])"));
}

function overflowsSideways() {
  const root = document.documentElement;
  return root.scrollWidth > root.clientWidth + 1;
}

/// Install the frame's wheel and key handling. Returns `{ dispose }`.
export function installInput({ zoomStep, zoomReset, pageStep, pageEdge, onfind, isPresenting }) {
  const lock = createAxisLock();

  function onwheel(event) {
    // Ctrl+wheel is a zoom gesture and `watchPinch` owns it.
    if (event.ctrlKey) return;
    if (event.metaKey || event.shiftKey || event.altKey) return;
    const dx = normalizeDelta(event.deltaX, event.deltaMode, window.innerWidth);
    const dy = normalizeDelta(event.deltaY, event.deltaMode, window.innerHeight);
    const axis = lock.next(event.timeStamp, dx, dy);
    if (!axis || !overflowsSideways()) return;
    // Only the off-axis part is taken away. A swipe that is vertical with a
    // little sideways drift would otherwise wobble the page left and right.
    if (axis === "y" && dx !== 0) {
      event.preventDefault();
      window.scrollBy({ top: dy, behavior: "instant" });
    } else if (axis === "x" && dy !== 0) {
      event.preventDefault();
      window.scrollBy({ left: dx, behavior: "instant" });
    }
  }

  function onkeydown(event) {
    if (event.defaultPrevented || typing(event.target)) return;
    const action = keyAction(
      { key: event.key, ctrl: event.ctrlKey, meta: event.metaKey, shift: event.shiftKey, alt: event.altKey },
      isPresenting(),
      overflowsSideways(),
    );
    if (!action) return;
    event.preventDefault();
    if (action.kind === "zoomStep") zoomStep(action.arg);
    else if (action.kind === "zoomReset") zoomReset();
    else if (action.kind === "find") onfind();
    else if (action.kind === "pageStep") pageStep(action.arg);
    else if (action.kind === "pageEdge") pageEdge(action.arg);
  }

  // Non-passive: preventDefault is the whole point.
  window.addEventListener("wheel", onwheel, { passive: false });
  document.addEventListener("keydown", onkeydown);

  return {
    dispose() {
      window.removeEventListener("wheel", onwheel);
      document.removeEventListener("keydown", onkeydown);
    },
  };
}
