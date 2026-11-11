// Which items of a tool band fit, and which move into the "..." menu.
import assert from "node:assert/strict";
import test from "node:test";
import { fitBand } from "../../src/lib/band-fit.js";

const item = (group, width = 30, pinned = false) => ({ group, width, pinned });
const fit = (items, available) => fitBand({ items, available, gap: 4, divider: 1, more: 30 });

test("everything shows when it fits, with no overflow", () => {
  const result = fit([item("a"), item("a"), item("b")], 500);
  assert.deepEqual(result.visible, [true, true, true]);
  assert.equal(result.overflowed, 0);
  assert.deepEqual(result.dividers, [false, false, true]);
});

test("the right-most item goes first and the menu button is reserved", () => {
  // Three items: 30*3 + 2 gaps = 98 fits in 100, but not once a divider is added.
  const result = fit([item("a"), item("a"), item("a")], 100);
  assert.equal(result.overflowed, 0);
  // Four items need 30*4 + 3*4 = 132; with the last gone: 98 + 4 + 30 = 132 still too wide.
  const narrower = fit([item("a"), item("a"), item("a"), item("a")], 100);
  assert.deepEqual(narrower.visible, [true, true, false, false]);
  assert.equal(narrower.overflowed, 2);
});

test("pinned items stay, and the next one to the left moves instead", () => {
  const result = fit([item("a"), item("a"), item("a", 30, true), item("a")], 100);
  assert.deepEqual(result.visible, [true, false, true, false]);
});

test("a divider disappears with the last item of its group", () => {
  const items = [item("a"), item("a"), item("b"), item("c")];
  const result = fit(items, 130);
  // 4 items + 3 gaps + 2 dividers (5 each) = 142 > 130; dropping "c" leaves 30*3+8+5+30+4 = 137, still too wide.
  assert.deepEqual(result.visible, [true, true, false, false]);
  assert.deepEqual(result.dividers, [false, false, false, false]);
});

test("nothing movable left is not an error", () => {
  const result = fit([item("a", 80, true), item("a", 80, true)], 100);
  assert.deepEqual(result.visible, [true, true]);
  assert.equal(result.overflowed, 0);
});
