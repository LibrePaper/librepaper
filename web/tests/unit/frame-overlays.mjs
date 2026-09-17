import assert from "node:assert/strict";
import { createFrameOverlays } from "../../src/lib/reader/frame-overlays.js";

let ready = false;
const sent = [];
const overlays = createFrameOverlays({ ready: () => ready, send: (message) => sent.push(message) });
const comments = [
  {
    id: "passage", start: 4, end: 9, motivation: "editing", proposed: "after",
    outcome: "", resolved: false,
    presentation: { rendered_exact: "quote", rendered_position_utf16: 4 },
  },
  {
    // A note left between two words: no quotation of its own, so the frame is
    // told to draw a mark rather than a range.
    id: "point", start: 7, end: 7, motivation: "commenting", resolved: false,
    presentation: { rendered_position_utf16: 7 },
  },
  { id: "orphan", start: 10, end: 12, orphaned: true, motivation: "commenting" },
];

assert.equal(overlays.annotations(comments), false);
assert.equal(overlays.selection("passage"), false);
assert.deepEqual(sent, []);

ready = true;
assert.equal(overlays.annotations(comments), true);
assert.deepEqual(sent, [
  { type: "regions", regions: [] },
  {
    type: "highlight",
    ranges: [
      {
        id: "passage", point: false, start: 4, end: 9, motivation: "editing",
        resolved: false, proposed: "after", outcome: "",
      },
      {
        id: "point", point: true, start: 7, end: 7, motivation: "commenting",
        resolved: false, proposed: "", outcome: "",
      },
    ],
  },
]);

assert.equal(overlays.annotations(comments), false);
assert.equal(overlays.selection("passage"), true);
assert.equal(overlays.selection("passage"), false);

overlays.resetAnnotations();
assert.equal(overlays.annotations(comments), true);
assert.equal(overlays.selection("passage"), false);

overlays.reset();
assert.equal(overlays.selection("passage"), true);

console.log("frame overlay tests passed");
