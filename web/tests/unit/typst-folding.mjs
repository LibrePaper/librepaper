import assert from "node:assert/strict";
import { test } from "node:test";
import { EditorState } from "@codemirror/state";
import { foldable } from "@codemirror/language";
import { byteToUtf16, foldRangesFrom, typstFolding, setFolds, foldsField, folder } from "../../src/lib/typst-folding.js";

test("byte offsets map to UTF-16 indexes", () => {
  const ascii = byteToUtf16("abc");
  assert.deepEqual([0, 1, 3, 9].map(ascii), [0, 1, 3, 3]);
  const text = "aé中😀b";
  const map = byteToUtf16(text);
  // a=1 byte, é=2, 中=3, 😀=4 (two UTF-16 units), b=1
  assert.deepEqual([0, 1, 3, 6, 10, 11, 99].map(map), [0, 1, 2, 3, 5, 6, 6]);
  assert.equal(map(2), 1); // inside é rounds down
});

test("ranges convert and single-line ones drop", () => {
  const source = "é #f[\n  x\n]\n#[a]\n";
  const open = Buffer.byteLength("é #f[");
  const close = Buffer.byteLength("é #f[\n  x\n");
  const answer = { ranges: [
    { from: open, to: close, kind: "region" },
    { from: Buffer.byteLength("é #f[\n  x\n]\n#["), to: Buffer.byteLength("é #f[\n  x\n]\n#[a"), kind: "region" },
    { from: 3, to: 3, kind: "region" },
  ] };
  const ranges = foldRangesFrom(source, answer);
  assert.deepEqual(ranges, [{ from: 5, to: 10 }]);
  assert.equal(source.slice(ranges[0].from, ranges[0].to), "\n  x\n");
});

test("heading and comment answers fold after the first line", () => {
  const source = "= A\ntext\n\n= B\n/* c\nd */\n";
  const heading = { from: 3, to: 9, kind: "heading" };
  const comment = { from: source.indexOf("/*") + 2, to: source.indexOf("*/"), kind: "comment" };
  const [h, c] = foldRangesFrom(source, { ranges: [heading, comment] });
  assert.equal(source.slice(h.from, h.to), "\ntext\n");
  assert.equal(source.slice(c.from, c.to), " c\nd ");
});

test("a null answer yields no folds", () => {
  assert.deepEqual(foldRangesFrom("a\nb", null), []);
  assert.deepEqual(foldRangesFrom("a\nb", {}), []);
});

function stateWith(doc) {
  return EditorState.create({ doc, extensions: [typstFolding({ request: async () => null })] });
}

test("the fold service offers the range starting on the line", () => {
  const doc = "#f[\n  x\n]\n";
  let state = stateWith(doc);
  assert.equal(foldable(state, 0, 4), null);
  state = state.update({ effects: setFolds.of([{ from: 4, to: 9 }]) }).state;
  assert.deepEqual(foldable(state, 0, 4), { from: 4, to: 9 });
  assert.equal(foldable(state, 5, 8), null);
});

test("folds follow an insertion before them", () => {
  let state = stateWith("#f[\n  x\n]\n");
  state = state.update({ effects: setFolds.of([{ from: 4, to: 9 }]) }).state;
  state = state.update({ changes: { from: 0, insert: "ab\n" } }).state;
  assert.deepEqual(state.field(foldsField), [{ from: 7, to: 12 }]);
  assert.deepEqual(foldable(state, 3, 7), { from: 7, to: 12 });
});

function harness(request) {
  const timers = [];
  let doc = { toString: () => "one\ntwo" };
  const applied = [];
  const job = folder({
    getDoc: () => doc,
    apply: (r) => applied.push(r),
    request,
    path: () => "main.typ",
    setTimer: (fn) => timers.push(fn) && timers.length,
    clearTimer: () => {},
  });
  return { job, timers, applied, change: () => { doc = { toString: () => "other" }; } };
}

test("an answer is applied when the document is unchanged", async () => {
  const h = harness(async () => ({ ranges: [{ from: 3, to: 7, kind: "region" }] }));
  h.job.schedule();
  await h.timers.shift()();
  assert.deepEqual(h.applied, [[{ from: 3, to: 7 }]]);
});

test("a stale answer is dropped and the request rescheduled", async () => {
  let h;
  h = harness(async () => { h.change(); return { ranges: [{ from: 3, to: 7, kind: "region" }] }; });
  h.job.schedule();
  await h.timers.shift()();
  assert.deepEqual(h.applied, []);
  assert.equal(h.timers.length, 1);
});

test("a failing request leaves folds empty", async () => {
  const h = harness(async () => { throw new Error("no worker"); });
  h.job.schedule();
  await h.timers.shift()();
  assert.deepEqual(h.applied, []);
});
