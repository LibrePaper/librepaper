// Two-finger zoom, kept from the browser.
//
// On a phone, a pinch anywhere in the app zooms the whole page, interface and
// all, instead of the document the reader is looking at. A page that can zoom
// itself calls `watchPinch` to turn the gesture into its own zoom. A page with
// nothing to zoom calls `blockPageZoom`, so the pinch does nothing at all.
// Every listener is added to the target and removed by `stop()`.

const WHEEL_IDLE_MS = 200;
// Two fingers on top of each other have no scale to measure.
const MIN_START_DISTANCE = 1;

function distance(a, b) {
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
}

/// Turn pinches and ctrl+wheel gestures on `target` into zoom callbacks.
///
/// `onchange(factor, x, y)` runs on every move and `onend(factor, x, y)` runs
/// once the gesture is over. `factor` is relative to the start of the gesture
/// and is not clamped. `(x, y)` is where the gesture started. Returns `stop()`.
export function watchPinch(target, { onchange = () => {}, onend = () => {} } = {}) {
  let start = null; // { distance, x, y } while a pinch is active
  let factor = 1;
  let wheel = null; // { x, y, factor, timer } during a burst of ctrl+wheel

  const endWheel = () => {
    const { x, y, factor: last } = wheel;
    wheel = null;
    onend(last, x, y);
  };

  const onTouchStart = (event) => {
    if (event.touches.length !== 2) return;
    const [a, b] = event.touches;
    const d = distance(a, b);
    if (d < MIN_START_DISTANCE) return;
    start = {
      distance: d,
      x: (a.clientX + b.clientX) / 2,
      y: (a.clientY + b.clientY) / 2,
    };
    factor = 1;
  };

  const onTouchMove = (event) => {
    if (!start || event.touches.length !== 2) return;
    event.preventDefault();
    const [a, b] = event.touches;
    factor = distance(a, b) / start.distance;
    onchange(factor, start.x, start.y);
  };

  const onTouchEnd = (event) => {
    if (!start || event.touches.length >= 2) return;
    const { x, y } = start;
    start = null;
    onend(factor, x, y);
  };

  // Safari reports pinches as gesture events on top of the touch events.
  // The touch events do the zooming, so these only refuse the page zoom.
  const onGesture = (event) => event.preventDefault();

  const onWheel = (event) => {
    if (!event.ctrlKey) return;
    event.preventDefault();
    if (!wheel) wheel = { x: event.clientX, y: event.clientY, factor: 1, timer: 0 };
    const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1);
    wheel.factor *= Math.exp(-delta / 100);
    onchange(wheel.factor, wheel.x, wheel.y);
    clearTimeout(wheel.timer);
    wheel.timer = setTimeout(endWheel, WHEEL_IDLE_MS);
  };

  const listeners = [
    ["touchstart", onTouchStart, { passive: true }],
    ["touchmove", onTouchMove, { passive: false }],
    ["touchend", onTouchEnd, { passive: true }],
    ["touchcancel", onTouchEnd, { passive: true }],
    ["gesturestart", onGesture, { passive: false }],
    ["gesturechange", onGesture, { passive: false }],
    ["gestureend", onGesture, { passive: false }],
    ["wheel", onWheel, { passive: false }],
  ];
  for (const [type, fn, options] of listeners) target.addEventListener(type, fn, options);

  return () => {
    for (const [type, fn] of listeners) target.removeEventListener(type, fn);
    if (wheel) clearTimeout(wheel.timer);
    wheel = null;
    start = null;
  };
}

/// Refuse page zoom on `target` without zooming anything. Returns `stop()`.
export function blockPageZoom(target) {
  return watchPinch(target);
}
