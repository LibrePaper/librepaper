// Adversarial checks for history content provenance. This file is intentionally
// not wired into the default browser checks: it documents the proof boundary
// for the controller to consume without rendering intermediate checkpoints.
import assert from "node:assert/strict";
import {
  MAX_PROVENANCE_INTERVALS,
  captureIntervalAuthorship,
  normalizeAuthorship,
  provenIntervals,
  refineSourceAttribution,
} from "../src/lib/provenance.js";

assert.deepEqual(normalizeAuthorship({ by: "publisher" }), { kind: "unknown" });
assert.deepEqual(normalizeAuthorship({ publisher: "alice", contributors: ["alice"] }), { kind: "unknown" });
assert.deepEqual(captureIntervalAuthorship({ by: "publisher", authorship: { kind: "single", name: "Alice" } }), { kind: "single", name: "Alice" });
assert.deepEqual(captureIntervalAuthorship({ by: "Alice" }), { kind: "unknown" });

const checkpoints = [
  { sha: "a" },
  { sha: "b", original_parent: "a", ancestry_gap: false, authorship: { kind: "single", name: "Alice" } },
  { sha: "c", original_parent: "b", ancestry_gap: false, authorship: { kind: "single", name: "Alice" } },
];
assert.equal(provenIntervals(checkpoints, "a", "c").length, 2);
assert.equal(provenIntervals(checkpoints.map((point) => ({ ...point, by: "publisher" })), "a", "c").length, 2);
assert.equal(provenIntervals(checkpoints.map((point) => ({ ...point, ancestry_gap: true })), "a", "c"), null);
assert.equal(provenIntervals(checkpoints, "a", null), null);

const ranges = (start, end) => [{ path: "main.md", start, end, validated: true }];
const intervals = [
  { sha: "b", original_parent: "a", authorship: { kind: "single", name: "Alice" }, sourceRanges: ranges(0, 5), sourceBytes: 5 },
  { sha: "c", original_parent: "b", authorship: { kind: "single", name: "Alice" }, sourceRanges: ranges(10, 15), sourceBytes: 5 },
];
const exact = refineSourceAttribution({
  checkpoints, baselineSha: "a", targetSha: "c", intervals,
  hunks: [{ id: "mapped", sourceRanges: ranges(0, 5) }, { id: "partial", sourceRanges: ranges(0, 6) }],
});
assert.equal(exact.hunks[0].who, "Alice");
assert.equal(exact.hunks[1].who, undefined);

const several = refineSourceAttribution({
  checkpoints, baselineSha: "a", targetSha: "c",
  intervals: [
    { ...intervals[0], sourceRanges: ranges(0, 3) },
    { ...intervals[1], authorship: { kind: "single", name: "Bob" }, sourceRanges: ranges(3, 5) },
  ],
  hunks: [{ sourceRanges: ranges(0, 5) }],
});
assert.equal(several.hunks[0].who, "several people");

const capped = refineSourceAttribution({
  checkpoints: Array.from({ length: MAX_PROVENANCE_INTERVALS + 2 }, (_, index) => ({
    sha: String(index), original_parent: String(index - 1), ancestry_gap: false,
    authorship: { kind: "single", name: "Alice" },
  })).map((point, index) => index === 0 ? { sha: "0" } : point),
  baselineSha: "0", targetSha: String(MAX_PROVENANCE_INTERVALS + 1), intervals: [], hunks: [{ id: "unchanged" }],
});
assert.equal(capped.refined, false);

console.log("provenance: adversarial contract documented");
