// The decisions behind the PDF frame's wheel and key handling, without a browser.

import assert from "node:assert/strict";
import {
  GESTURE_GAP,
  createAxisLock,
  dominantAxis,
  keyAction,
  normalizeDelta,
} from "../../src/lib/pdf/input.js";

assert.equal(normalizeDelta(10, 0, 800), 10);
assert.equal(normalizeDelta(3, 1, 800), 48, "lines are 16px");
assert.equal(normalizeDelta(1, 2, 800), 800, "pages are a window high");

assert.equal(dominantAxis(0, 10), "y");
assert.equal(dominantAxis(10, 0), "x");
assert.equal(dominantAxis(5, 10), "y", "exactly twice counts");
assert.equal(dominantAxis(6, 10), null, "a diagonal has no axis yet");
assert.equal(dominantAxis(0, 0), null);

// A gesture locks on its first clear event and keeps the lock across a pause
// shorter than the gap; a longer pause starts again.
{
  const lock = createAxisLock();
  assert.equal(lock.next(1000, 5, 6), null, "undecided");
  assert.equal(lock.next(1010, 1, 12), "y", "locks");
  assert.equal(lock.next(1020, 30, 3), "y", "stays locked");
  assert.equal(lock.next(1020 + GESTURE_GAP, 30, 3), "y", "a pause of exactly the gap keeps it");
  assert.equal(lock.next(1020 + GESTURE_GAP * 2 + 1, 30, 3), "x", "a longer pause starts over");
}

const key = (name, mods = {}) => ({ key: name, ctrl: false, meta: false, shift: false, alt: false, ...mods });

assert.deepEqual(keyAction(key("=", { ctrl: true }), false, false), { kind: "zoomStep", arg: 1.1 });
assert.deepEqual(keyAction(key("+", { meta: true }), false, false), { kind: "zoomStep", arg: 1.1 });
assert.deepEqual(keyAction(key("-", { ctrl: true }), false, false), { kind: "zoomStep", arg: 1 / 1.1 });
assert.deepEqual(keyAction(key("0", { ctrl: true }), false, false), { kind: "zoomReset" });
assert.deepEqual(keyAction(key("f", { ctrl: true }), false, false), { kind: "find" });
assert.deepEqual(keyAction(key("F", { meta: true }), false, false), { kind: "find" });
assert.equal(keyAction(key("f", { ctrl: true, alt: true }), false, false), null, "alt is ignored");
assert.equal(keyAction(key("c", { ctrl: true }), false, false), null);

// Never ours: frame.js has them.
for (const name of ["c", "C", "Escape"]) {
  assert.equal(keyAction(key(name), false, false), null, name);
  assert.equal(keyAction(key(name), true, false), null, `${name} while presenting`);
}

assert.deepEqual(keyAction(key("ArrowRight"), false, false), { kind: "pageStep", arg: 1 });
assert.deepEqual(keyAction(key("ArrowLeft"), false, false), { kind: "pageStep", arg: -1 });
assert.equal(keyAction(key("ArrowRight"), false, true), null, "sideways scrolling keeps the arrows");
assert.equal(keyAction(key("ArrowDown"), false, false), null, "down is the browser's when not presenting");
assert.deepEqual(keyAction(key("Home"), false, true), { kind: "pageEdge", arg: "first" });
assert.deepEqual(keyAction(key("End"), false, false), { kind: "pageEdge", arg: "last" });

for (const name of ["ArrowDown", "ArrowRight", "PageDown", " "]) {
  assert.deepEqual(keyAction(key(name), true, false), { kind: "pageStep", arg: 1 }, name);
}
for (const name of ["ArrowUp", "ArrowLeft", "PageUp", "Backspace"]) {
  assert.deepEqual(keyAction(key(name), true, false), { kind: "pageStep", arg: -1 }, name);
}
assert.deepEqual(keyAction(key("ArrowRight"), true, true), { kind: "pageStep", arg: 1 }, "presenting ignores overflow");
