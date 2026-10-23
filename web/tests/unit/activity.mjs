// The activity module is now only `minutesIn`, the minute a version falls at
// in its day, which places a version on its day's axis.
import assert from "node:assert/strict";
import { minutesIn } from "../../src/lib/activity.js";

assert.equal(minutesIn("2026-09-10T09:05:00Z", "UTC"), 545);
// The reader's own timezone decides the minute, and the day it falls in.
assert.equal(minutesIn("2026-09-10T09:05:00Z", "America/Toronto"), 305);
assert.equal(minutesIn("2026-09-10T23:30:00Z", "Asia/Tokyo"), 510);
// Midnight is hour zero, not hour twenty-four.
assert.equal(minutesIn("2026-09-10T00:00:00Z", "UTC"), 0);
assert.equal(minutesIn("not a time", "UTC"), -1);
console.log("activity: a version's minute in the reader's own day");
