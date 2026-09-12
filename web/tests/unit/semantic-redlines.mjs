// Contract checks for the frame-side semantic redline payload. These are pure
// checks; DOM rebinding is exercised by the browser reader checks.
import assert from "node:assert/strict";
import { projectText, projectionHunks } from "../../src/lib/diff-display.js";
import {
  SEMANTIC_REDLINE_VERSION,
  displayOffset,
  targetSemanticItems,
  validateSemanticRedlinePayload,
} from "../../src/agent/semantic-redlines.js";

const baseline = projectText("A short paragraph.");
const target = projectText("A changed paragraph.");
const [hunk] = projectionHunks(baseline, target);
const payload = {
  version: SEMANTIC_REDLINE_VERSION,
  generation: 3,
  frameGeneration: 9,
  baselineProjection: baseline,
  targetProjection: target,
  hunks: [hunk],
};

assert.equal(validateSemanticRedlinePayload(payload), true);
assert.equal(displayOffset(target, hunk.fromB), hunk.position);
assert.ok(targetSemanticItems(payload).some((item) => item.id === "semantic-hunk-0"));

const malformed = { ...payload, hunks: [{ ...hunk, inserted: [] }] };
assert.equal(validateSemanticRedlinePayload(malformed), false);
assert.equal(validateSemanticRedlinePayload({ ...payload, generation: -1 }), false);
assert.equal(validateSemanticRedlinePayload({ ...payload, targetProjection: { ...target, text: "tampered" } }), false);
assert.equal(validateSemanticRedlinePayload({ ...payload, targetProjection: { ...target, complete: false } }), false, "incomplete frame projections are rejected");

console.log("semantic-redlines: ok");
