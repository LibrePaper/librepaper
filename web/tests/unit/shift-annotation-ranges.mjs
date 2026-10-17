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
