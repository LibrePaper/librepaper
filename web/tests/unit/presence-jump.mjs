// Where a collaborator's avatar sends the reader. The session supplies the
// presence file and the resolved caret; these cases pin which file wins and
// what range is shown.
import assert from "node:assert/strict";
import { test } from "node:test";
import { presenceJumpTarget } from "../../src/lib/presence-jump.js";

test("nothing known gives no target", () => {
  assert.equal(presenceJumpTarget({}), null);
  assert.equal(presenceJumpTarget({ file: "", caret: null }), null);
});

test("a presence file with no caret opens the file and shows no range", () => {
  assert.deepEqual(presenceJumpTarget({ file: "f1" }), { file: "f1", from: null, to: null });
  assert.deepEqual(presenceJumpTarget({ file: "f1", caret: null }), { file: "f1", from: null, to: null });
});

test("a caret without a file falls back to the presence file", () => {
  const caret = { file: "", anchor: 4 };
  assert.deepEqual(presenceJumpTarget({ file: "f2", caret }), { file: "f2", from: null, to: null });
});

test("the caret's file wins over the presence record", () => {
  const caret = { file: "f3", anchor: 10, head: 10 };
  assert.deepEqual(presenceJumpTarget({ file: "f1", caret }), { file: "f3", from: 10, to: 10 });
});

test("a selection is returned in order, whichever way it was made", () => {
  assert.deepEqual(presenceJumpTarget({ caret: { file: "a", anchor: 9, head: 3 } }), { file: "a", from: 3, to: 9 });
  assert.deepEqual(presenceJumpTarget({ caret: { file: "a", anchor: 3, head: 9 } }), { file: "a", from: 3, to: 9 });
});

test("a missing head is the anchor", () => {
  assert.deepEqual(presenceJumpTarget({ caret: { file: "a", anchor: 5 } }), { file: "a", from: 5, to: 5 });
});

test("negative or non-numeric offsets are clamped or ignored", () => {
  assert.deepEqual(presenceJumpTarget({ file: "f1", caret: { file: "a", anchor: -2, head: -7 } }), { file: "a", from: 0, to: 0 });
  assert.deepEqual(presenceJumpTarget({ file: "f1", caret: { file: "a", anchor: Number.NaN } }), { file: "f1", from: null, to: null });
});
