// Where the bar over a selection goes.
//
// Only the arithmetic. Measuring the selection, the frame and the bar itself
// is the page's -- all three are things on a screen -- and this decides what
// to do with the numbers, which is the part worth checking without one.
//
// Three rules, and all are about not putting the bar somewhere it cannot be
// used: it never runs off either edge of the window, it never rises under
// the bar at the top of the page, which would put it behind the one thing
// that is always on top, and it never sinks below the bottom of what can be
// seen, which is where a phone's keyboard or a selection scrolled out of view
// would otherwise leave it.

/// The gap kept at the window's edges.
export const MARGIN = 8;

/// How far above the selection the bar sits, when there is room for it.
export const LIFT = 42;

/// Keep a bar of `width` within a window of `windowWidth`.
///
/// The left edge wins when the two rules disagree, which is what happens when
/// the bar is wider than the window: better to lose the right-hand end, which
/// is where the least-used control is, than the left-hand one.
export function withinWindow(left, width, windowWidth, margin = MARGIN) {
  return Math.max(margin, Math.min(windowWidth - width - margin, left));
}

/// Where to put a bar of `width` over `rect`, a selection measured inside a
/// frame whose own position is `frame`.
///
/// `minTop` is the highest the bar may start: the height of the page's top bar
/// plus the gap below it, which the caller reads from the stylesheet that
/// sets it rather than from a number copied out of it.
///
/// `maxTop` is the lowest it may start: the bottom of the visible area less
/// the bar's own height and a margin, so the bar stays on screen when the
/// selection is near the bottom or below it. It is applied before `minTop`,
/// so when a window is too short for both the bar stays clear of the top bar.
///
/// Answers the point it centred on as well as the placing, because the width
/// it was given is a guess until the bar is drawn and the centring has to be
/// made good against the same point once it is.
export function placeBar({
  rect,
  frame,
  width,
  minTop,
  maxTop = Number.POSITIVE_INFINITY,
  windowWidth,
  margin = MARGIN,
  lift = LIFT,
}) {
  const center = frame.left + rect.left + (rect.right - rect.left) / 2;
  return {
    center,
    left: withinWindow(center - width / 2, width, windowWidth, margin),
    top: Math.max(minTop, Math.min(maxTop, frame.top + rect.top - lift)),
  };
}

/// The bar is placed before it is drawn, so it is placed at a guessed width;
/// this re-centres it on the same point once the real one is known. Answers
/// the corrected left edge, or null when the difference is too small to be
/// worth a redraw.
///
/// The threshold is not a micro-optimization: this runs from an effect that
/// reads the width it is about to change the position of, and a correction
/// that always "changes" something is a loop.
export function recenteredLeft({ center, left, width, windowWidth, margin = MARGIN }) {
  const corrected = withinWindow(center - width / 2, width, windowWidth, margin);
  return Math.abs(corrected - left) > 1 ? corrected : null;
}
