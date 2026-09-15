// Where the bar over a selection goes.
//
// Only the arithmetic. Measuring the selection, the frame and the bar itself
// is the page's -- all three are things on a screen -- and this decides what
// to do with the numbers, which is the part worth checking without one.
//
// Two rules, and both are about not putting the bar somewhere it cannot be
// used: it never runs off either edge of the window, and it never rises under
// the bar at the top of the page, which would put it behind the one thing
// that is always on top.

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
/// `minTop` is the lowest the bar may start: the height of the page's top bar
/// plus the gap below it, which the caller reads from the stylesheet that
/// sets it rather than from a number copied out of it.
export function placeBar({ rect, frame, width, minTop, windowWidth, margin = MARGIN, lift = LIFT }) {
  const middle = frame.left + rect.left + (rect.right - rect.left) / 2;
  return {
    left: withinWindow(middle - width / 2, width, windowWidth, margin),
    top: Math.max(minTop, frame.top + rect.top - lift),
  };
}

/// The bar is placed before it is drawn, and a bar is only as wide as what is
/// in it -- highlighting adds five swatches -- so the placing is made good
/// once the real width is known. Answers the corrected left edge, or null
/// when the difference is too small to be worth a redraw.
///
/// The threshold is not a micro-optimization: this runs from an effect that
/// reads the width it is about to change the position of, and a correction
/// that always "changes" something is a loop.
export function correctedLeft({ left, width, windowWidth, margin = MARGIN }) {
  const corrected = withinWindow(left, width, windowWidth, margin);
  return Math.abs(corrected - left) > 1 ? corrected : null;
}
