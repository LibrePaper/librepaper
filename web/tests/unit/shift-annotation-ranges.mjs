import assert from "node:assert/strict";
import { shiftAnnotationRanges } from "../../src/lib/shift-annotation-ranges.js";

const ranges = [
  { id: "crossing", start: 1, end: 8 },
  { id: "after", start: 8, end: 10 },
  { id: "before", start: 0, end: 2 },
];
assert.deepEqual(
  shiftAnnotationRanges("abcdefghij", "abXYZQfghij", ranges),
  [
    { id: "crossing", start: 1, end: 9 },
    { id: "after", start: 9, end: 11 },
    { id: "before", start: 0, end: 2 },
  ],
);
assert.deepEqual(
  shiftAnnotationRanges("abcdef", "abXcdef", [{ id: "ending-at-edit", start: 0, end: 2 }, { id: "crossing", start: 1, end: 4 }]),
  [{ id: "ending-at-edit", start: 0, end: 2 }, { id: "crossing", start: 1, end: 5 }],
);
const zeroWidth = shiftAnnotationRanges("abcdef", "abXcdef", [{ id: "caret", start: 2, end: 2 }])[0];
assert.equal(zeroWidth.start, zeroWidth.end, "zero-width offsets remain ordered through an insertion");
assert.deepEqual(
  shiftAnnotationRanges("abcdef", "abXcdef", [{ id: "grows", start: 0, end: 2, grow: true }]),
  [{ id: "grows", start: 0, end: 3, grow: true }],
);
assert.deepEqual(
  shiftAnnotationRanges("abcdef", "Xabcdef", [{ id: "grows-left", start: 0, end: 2, grow: true }]),
  [{ id: "grows-left", start: 0, end: 3, grow: true }],
);
assert.deepEqual(
  shiftAnnotationRanges("abcdef", "abef", [{ id: "grows-deleted", start: 0, end: 2, grow: true }]),
  [{ id: "grows-deleted", start: 0, end: 2, grow: true }],
);
assert.deepEqual(
  shiftAnnotationRanges("abcdef", "abcdXef", [{ id: "grows-apart", start: 0, end: 2, grow: true }]),
  [{ id: "grows-apart", start: 0, end: 2, grow: true }],
);
assert.deepEqual(
  shiftAnnotationRanges("abcdef", "abcdXef", [{ id: "grows-after", start: 5, end: 6, grow: true }]),
  [{ id: "grows-after", start: 6, end: 7, grow: true }],
);
assert.deepEqual(
  shiftAnnotationRanges("abcdef", "abcdXef", [{ id: "grows-at-start", start: 4, end: 6, grow: true }]),
  [{ id: "grows-at-start", start: 4, end: 7, grow: true }],
);
