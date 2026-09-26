// What is left of the activity module once its work-volume half is gone:
// the five-shade scale the version calendar shades a day's version count
// with. See src/lib/activity.js for why `withWork` is not here any more.

import assert from "node:assert/strict";
import { level } from "../../src/lib/activity.js";

// --- five shades, zero reserved ---------------------------------------------

assert.equal(level(0, 100), 0, "nothing happened is its own shade");
assert.equal(level(1, 100), 1, "one version is not nothing");
assert.equal(level(100, 100), 4);
assert.equal(level(51, 100), 3);
assert.equal(level(5, 0), 1, "a count with nothing to compare against still shows");

console.log("activity: five shades passed");
