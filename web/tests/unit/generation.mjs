// The "am I still the current run?" primitive that seven modules in
// lib/reader share.
//
// What is checked is the thing the hand-rolled copies could get wrong: that a
// run overtaken by a newer one goes stale and stays stale, that the newest run
// never calls itself stale, and that `mark()` follows the run in progress
// rather than starting one -- the distinction local-preview depends on, where
// `start()` must notice a `stop()` that happened while it was awaiting, but
// must not invalidate the start it is itself performing.
import assert from "node:assert/strict";
import { createGeneration } from "../../src/lib/reader/generation.js";

const runs = createGeneration();

const first = runs.begin();
assert.equal(first(), false, "the run just begun is the current one");

const second = runs.begin();
assert.equal(first(), true, "an overtaken run is stale");
assert.equal(second(), false);
assert.equal(first(), true, "and stays stale when asked again");

// Teardown, or an explicit invalidation: everything in flight is abandoned and
// nothing is begun in its place.
runs.cancel();
assert.equal(second(), true, "cancel abandons the run in flight");

const third = runs.begin();
const alongside = runs.mark();
assert.equal(alongside(), false, "mark follows the current run");
assert.equal(third(), false, "and does not displace it");

runs.begin();
assert.equal(alongside(), true, "a new run makes the marked one stale too");

// Two generations are independent: one module's teardown says nothing about
// another's.
const others = createGeneration();
const elsewhere = others.begin();
runs.cancel();
assert.equal(elsewhere(), false, "generations do not share a counter");
