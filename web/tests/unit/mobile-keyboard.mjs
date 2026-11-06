import assert from "node:assert/strict";
import test from "node:test";
import { isEditable, KEYBOARD_THRESHOLD, keyboardInset, watchKeyboard } from "../../src/lib/mobile/keyboard.js";

test("the keyboard counts as open from a 120 pixel gap", () => {
  assert.equal(KEYBOARD_THRESHOLD, 120);
});

test("keyboardInset is the part of the window the viewport no longer shows", () => {
  assert.equal(keyboardInset(800, { height: 800, offsetTop: 0, scale: 1 }), 0);
  assert.equal(keyboardInset(800, { height: 400, offsetTop: 0, scale: 1 }), 400);
  // When the page has been scrolled inside the layout viewport, offsetTop
  // moves the visible part down and the covered part shrinks by the same amount.
  assert.equal(keyboardInset(800, { height: 400, offsetTop: 50, scale: 1 }), 350);
  // A fraction of a pixel rounds, rather than leaving a fractional CSS length.
  assert.equal(keyboardInset(800.4, { height: 400.2, offsetTop: 0, scale: 1 }), 400);
  // Never negative, even when the viewport reports more than the window.
  assert.equal(keyboardInset(800, { height: 900, offsetTop: 0, scale: 1 }), 0);
});

test("keyboardInset reads nothing from a missing viewport or a pinch zoom", () => {
  assert.equal(keyboardInset(800, null), 0);
  assert.equal(keyboardInset(800, undefined), 0);
  // A pinch zoom shrinks the visual viewport too, and that is not the keyboard.
  assert.equal(keyboardInset(800, { height: 400, offsetTop: 0, scale: 2 }), 0);
  assert.equal(keyboardInset(800, { height: 400, offsetTop: 0, scale: 1.005 }), 400);
});

test("isEditable accepts text fields and content-editable elements", () => {
  assert.equal(isEditable({ tagName: "TEXTAREA" }), true);
  assert.equal(isEditable({ tagName: "INPUT", type: "text" }), true);
  assert.equal(isEditable({ tagName: "INPUT", type: "search" }), true);
  assert.equal(isEditable({ tagName: "DIV", isContentEditable: true }), true);
});

test("isEditable refuses inputs that never take text, and other elements", () => {
  for (const type of ["button", "checkbox", "radio", "submit", "range", "color", "file"]) {
    assert.equal(isEditable({ tagName: "INPUT", type }), false, type);
  }
  assert.equal(isEditable({ tagName: "DIV", isContentEditable: false }), false);
  assert.equal(isEditable({ tagName: "BUTTON" }), false);
  assert.equal(isEditable(null), false);
  assert.equal(isEditable(undefined), false);
});

test("watchKeyboard does nothing without a visual viewport", () => {
  const previous = globalThis.window;
  globalThis.window = { visualViewport: null };
  try {
    const calls = [];
    const stop = watchKeyboard((open, inset) => calls.push([open, inset]));
    assert.equal(typeof stop, "function");
    stop();
    assert.deepEqual(calls, []);
  } finally {
    if (previous === undefined) delete globalThis.window;
    else globalThis.window = previous;
  }
});
