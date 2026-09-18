// What the activity graph is drawn from: growth rather than size, five
// shades, and a day that is the reader's own. The calendar that draws them
// is checked next door, in history-calendar.mjs.
//
// The rows are the server's, so they are written here the way the endpoint
// writes them: `state_bytes` is the size of the document's whole history at
// the end of that minute, not the weight of the edits in it.

import assert from "node:assert/strict";
import { level, withWork } from "../../src/lib/activity.js";

const row = (at, changes, state_bytes, frontier = "") => ({ at, changes, state_bytes, frontier, peer: "" });

// --- work is growth ---------------------------------------------------------

{
  const rows = [
    row("2026-09-14T10:00:00Z", 3, 1000),
    row("2026-09-14T10:01:00Z", 2, 1400),
    row("2026-09-14T10:02:00Z", 1, 1200),
  ];
  const worked = withWork(rows);
  assert.deepEqual(worked.map((one) => one.work), [1000, 400, 0],
    "work is the growth since the row before, and a deletion is not negative work");
  assert.deepEqual(worked.map((one) => one.changes), [3, 2, 1],
    "a minute somebody spent deleting is still a minute somebody was there");
  // Rows arrive from the server oldest first; shaping must not depend on it.
  assert.deepEqual(withWork([...rows].reverse()).map((one) => one.work), [1000, 400, 0]);
}

// --- five shades, zero reserved ---------------------------------------------

assert.equal(level(0, 100), 0, "nothing happened is its own shade");
assert.equal(level(1, 100), 1, "one keystroke is not nothing");
assert.equal(level(100, 100), 4);
assert.equal(level(51, 100), 3);
assert.equal(level(5, 0), 1, "work with nothing to compare against still shows");

console.log("activity: growth and five shades passed");
