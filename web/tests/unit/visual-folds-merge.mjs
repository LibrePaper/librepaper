// Merging the folds a format finds: same-kind folds separated by whitespace
// become one pill, includes stay apart, and the detail stays one short line.

import { test } from "node:test";
import assert from "node:assert/strict";

import { mergeFolds } from "../../src/lib/visual/merge-folds.js";

test("same-kind folds separated by blank lines merge", () => {
  const text = "aaaa\n\nbbbb";
  const merged = mergeFolds(text, [
    { from: 0, to: 4, kind: "comment", detail: "x" },
    { from: 6, to: 10, kind: "comment", detail: "y" },
  ]);
  assert.deepEqual(merged, [{ from: 0, to: 10, kind: "comment", detail: "x · y" }]);
});

test("different kinds do not merge", () => {
  const text = "aaaa\n\nbbbb";
  const merged = mergeFolds(text, [
    { from: 0, to: 4, kind: "comment" },
    { from: 6, to: 10, kind: "definitions" },
  ]);
  assert.equal(merged.length, 2);
});

test("includes never merge", () => {
  const text = "\\input{a}\n\n\\input{b}";
  const merged = mergeFolds(text, [
    { from: 0, to: 9, kind: "include", target: "a" },
    { from: 11, to: text.length, kind: "include", target: "b" },
  ]);
  assert.deepEqual(
    merged.map((fold) => fold.target),
    ["a", "b"],
  );
});

test("non-whitespace between folds prevents merging", () => {
  const text = "aaaa\nX\nbbbb";
  const merged = mergeFolds(text, [
    { from: 0, to: 4, kind: "comment" },
    { from: 7, to: text.length, kind: "comment" },
  ]);
  assert.equal(merged.length, 2);
});

test("merged details are joined with a middle dot and cut to 80 characters", () => {
  const text = "aaaa\n\nbbbb";
  const first = "a".repeat(50);
  const second = "b".repeat(50);
  const [fold] = mergeFolds(text, [
    { from: 0, to: 4, kind: "comment", detail: first },
    { from: 6, to: 10, kind: "comment", detail: second },
  ]);
  assert.equal(fold.detail.length, 80);
  assert.equal(fold.detail, `${first} · ${second}`.slice(0, 79) + "…");
});

test("a fold without detail gets no detail key", () => {
  const [fold] = mergeFolds("aaaa", [{ from: 0, to: 4, kind: "preamble" }]);
  assert.equal("detail" in fold, false);
});
