import assert from "node:assert/strict";
import { createFrameOverlays } from "../../src/lib/reader/frame-overlays.js";

let ready = false;
const sent = [];
const overlays = createFrameOverlays({ ready: () => ready, send: (message) => sent.push(message) });
const comments = [
  {
    id: "passage", start: 4, end: 9, motivation: "editing", proposed: "after",
    outcome: "", resolved: false, point: false,
  },
  {
    id: "figure", motivation: "commenting", resolved: true,
    region: { image_digest: "abc", image_index: 2, x: 1, y: 2, w: 3, h: 4 },
  },
  { id: "orphan", start: 10, end: 12, orphaned: true, motivation: "commenting" },
];

assert.equal(overlays.annotations(comments), false);
assert.equal(overlays.selection("passage"), false);
assert.deepEqual(sent, []);

ready = true;
assert.equal(overlays.annotations(comments), true);
assert.deepEqual(sent, [
  {
    type: "regions",
    regions: [{
      id: "figure", point: false, digest: "abc", index: 2,
      x: 1, y: 2, w: 3, h: 4, motivation: "commenting", resolved: true,
    }],
  },
  {
    type: "highlight",
    ranges: [{
      id: "passage", point: false, start: 4, end: 9, motivation: "editing",
      resolved: false, proposed: "after", outcome: "",
    }],
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
