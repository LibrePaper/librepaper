// The phone bar's hide-on-scroll rule. Only the pure rule is checked here; the
// listeners are thin wrappers around the same rule.
import assert from "node:assert/strict";
import { barHiddenAfter } from "../../src/lib/bar-scroll.js";

// At the top the bar always shows.
assert.equal(barHiddenAfter(false, 10, 0), false);
assert.equal(barHiddenAfter(true, 10, 0), false);

// Tiny moves keep whatever state the bar is in.
assert.equal(barHiddenAfter(false, 2, 100), false);
assert.equal(barHiddenAfter(true, -2, 100), true);

// Scrolling down hides the bar only once the page is past 48 pixels.
assert.equal(barHiddenAfter(false, 10, 40), false);
assert.equal(barHiddenAfter(false, 10, 100), true);

// Scrolling up shows it again.
assert.equal(barHiddenAfter(true, -10, 100), false);

// Near the bottom the bar holds still: hiding it grows the page, the browser pulls the scroll back, and that must not read as a scroll up.
assert.equal(barHiddenAfter(true, -48, 500, 0), true);
assert.equal(barHiddenAfter(false, 10, 500, 20), false);
assert.equal(barHiddenAfter(true, -10, 500, -30), true);
assert.equal(barHiddenAfter(true, -10, 500, 200), false);
assert.equal(barHiddenAfter(true, -10, 0, 0), false);

console.log("bar-scroll: ok");
