// Pure contract checks for the semantic history projection/diff fallback.
// Kept separate from browser checks: renderers and DOMParser are intentionally
// not required to exercise bounds, Unicode offsets, and cache invalidation.
import assert from "node:assert/strict";
import {
  PROJECTION_VERSION,
  ProjectionCache,
  diffProjections,
  projectText,
  projectionHunks,
  validateProjection,
} from "../../src/lib/diff-display.js";

const unicode = projectText("e\u0301 🇨🇦 👩‍🔬.");
assert.equal(unicode.text, "e\u0301 🇨🇦 👩‍🔬.");
assert.deepEqual(unicode.tokens.filter((token) => token.kind === "grapheme").map((token) => token.value), ["🇨🇦", "👩‍🔬", "."]);
assert.equal(unicode.locations.find((location) => location.token === unicode.tokens.findIndex((token) => token.value === "👩‍🔬"))?.start, 8);
assert.equal(validateProjection(unicode), true);
assert.equal(validateProjection({ ...unicode, complete: false }), false, "incomplete projections cannot be compared");
assert.equal(validateProjection({ ...unicode, tokens: unicode.tokens.map((token, index) => index === 0 ? { ...token, comparable: false } : token) }), false, "unknown tokens cannot claim a complete projection");
assert.equal(validateProjection(projectText("x".repeat(9 * 1024 * 1024))), false, "aggregate projection text is bounded");
assert.equal(validateProjection({ ...unicode, locations: [{ ...unicode.locations[0], path: [0, -1] }] }), false, "projection paths require bounded nonnegative indices");
assert.equal(validateProjection({ ...unicode, locations: [{ ...unicode.locations[0], parts: [{ path: [0], sourceStart: 4, sourceEnd: 3 }] }] }), false, "projection parts require ordered offsets");
assert.equal(validateProjection({ ...unicode, locations: [{ ...unicode.locations[0], parts: [null] }] }), false, "malformed projection parts are rejected without throwing");
assert.equal(validateProjection({ ...unicode, assetEvidence: { paths: { "fig.png": { digest: "not-a-digest" } } } }), false, "asset evidence requires a verified digest");
assert.equal(validateProjection({ ...unicode, assetEvidence: { paths: { "fig.png": { digest: "d".repeat(64) } } } }), true, "verified asset evidence is bounded and accepted");

const before = projectText("A café 👩‍🔬.\nA second paragraph.");
const after = projectText("A café 👩‍🔬!\nA new second paragraph.");
assert.equal(before.version, PROJECTION_VERSION);
assert.equal(validateProjection(before), true);
assert.equal(validateProjection(after), true);
const hunks = projectionHunks(before, after);
assert.ok(hunks.some((hunk) => hunk.old === "." && hunk.insert === "!"));
assert.ok(hunks.some((hunk) => hunk.insert.includes("new")));

const bounded = diffProjections(projectText("x".repeat(100)), projectText("y".repeat(100)), { limits: { work: 0 } });
assert.equal(bounded.length, 1);
assert.equal(bounded[0].simplified, true);
const unknown = { kind: "math", value: "same-placeholder", display: "Equation", offset: 8, comparable: false };
assert.equal(diffProjections({ tokens: [unknown] }, { tokens: [{ ...unknown }] }).length, 1);
assert.equal(diffProjections({ tokens: [unknown] }, { tokens: [{ ...unknown }] })[0].simplified, false);

const cache = new ProjectionCache(1);
cache.setScope("private-a");
cache.put({ tree: "one" }, before);
assert.equal(cache.get({ tree: "one" }), before);
cache.put({ tree: "two" }, after);
assert.equal(cache.get({ tree: "one" }), undefined);
cache.setScope("private-b");
assert.equal(cache.get({ tree: "two" }), undefined);

console.log("diff-display: ok");
