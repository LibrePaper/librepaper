// The size arithmetic behind a pinch over the source. The gesture itself is
// wired to the DOM in attachEditorZoom and is checked in the browser.
import assert from "node:assert/strict";

const values = new Map();
globalThis.localStorage = {
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => values.set(key, value),
};

const { clampSize, sizeAfter, storedSize } = await import("../../src/lib/editor-zoom.js");
const KEY = "librepaper-editor-size";

assert.equal(clampSize(14), 14);
assert.equal(clampSize(3), 10);
assert.equal(clampSize(99), 28);
assert.equal(clampSize(NaN), 14);
assert.equal(clampSize("x"), 14);

assert.equal(sizeAfter(14, 2), 28);
assert.equal(sizeAfter(14, 0.5), 10);
assert.equal(sizeAfter(14, 1.5), 21);

assert.equal(storedSize(), 14);
values.set(KEY, JSON.stringify(17.6));
assert.equal(storedSize(), 18);
values.set(KEY, JSON.stringify(50));
assert.equal(storedSize(), 28);
values.set(KEY, "not json");
assert.equal(storedSize(), 14);

console.log("editor-zoom: all checks passed");
