// fragmentRangesFor maps the fragments a tracked pass created to the deletions
// that made them. The Rust twin, fragment_ranges in the room crate, runs the
// same fixture. Offsets are UTF-16 here: a fragment's `at` is the gap in the
// text after the edits, and the Rust test converts it to code points.
//
// The fixture holds no case where the author's own pending insertion is deleted
// together with original text around it (Kata b4bb). The planner drops the
// pending text without a fragment and stores the fragment of the rest, so the
// deleted text never equals the fragment, on both sides. Whatever the fix, it
// is pinned by a case added then.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fragmentRangesFor } from "../../src/lib/anchors.js";

const cases = JSON.parse(readFileSync(new URL("../fixtures/fragment-ranges.json", import.meta.url), "utf8"));

function applyEdits(text, edits) {
  let out = text;
  let adj = 0;
  for (const e of edits) {
    out = out.slice(0, e.from + adj) + e.insert + out.slice(e.to + adj);
    adj += e.insert.length - (e.to - e.from);
  }
  return out;
}

for (const c of cases) {
  assert.equal(applyEdits(c.before, c.edits), c.after, `${c.name}: after text`);
  assert.deepEqual(fragmentRangesFor(c.before, c.edits, structuredClone(c.fragments)), c.expect, c.name);
}

console.log(`fragment-ranges: ${cases.length} cases ok`);
