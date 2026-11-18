import assert from "node:assert/strict";
import { parseSourcepos } from "../../src/lib/sourcepos.js";

// Tagged: tag, line and column at both ends. The tag names the source.
assert.deepEqual(parseSourcepos("2:14:3-2:16:9"), { tag: 2, from: 14, to: 16 });
assert.deepEqual(parseSourcepos("0:1:1-0:1:20"), { tag: 0, from: 1, to: 1 });
// Untagged: line and column at both ends; the one source is tag 0.
assert.deepEqual(parseSourcepos("4:1-9:30"), { tag: 0, from: 4, to: 9 });
assert.deepEqual(parseSourcepos("12:0-12:5"), { tag: 0, from: 12, to: 12 });

// Malformed shapes are not positions at all.
for (const value of [
  "", " ", "3", "3:4", "3:4-5", "3:4:5", "3:4:5-6:7", "2:3-4:5:6",
  "a:1-2:3", "1:a-2:3", "1:2-3:x", "2:3-4:5-6", "1:2:3:4-5:6:7:8",
  "2:3:4-5:6", "1.5:2-3:4", "1:2-3:4 ", " 1:2-3:4",
]) {
  assert.equal(parseSourcepos(value), null, JSON.stringify(value));
}

// Negative numbers and line 0 are not 1-based lines.
for (const value of ["-1:2-3:4", "1:-2-3:4", "1:2--3:4", "-1:2:3-1:2:4", "0:0-1:1", "0:1-0:2", "1:2-0:4", "1:0:1-1:0:2"]) {
  assert.equal(parseSourcepos(value), null, JSON.stringify(value));
}

// Both ends of a tagged attribute must name the same source.
assert.equal(parseSourcepos("1:4:1-2:6:1"), null);

// Empty, missing and non-string values are no position.
for (const value of [null, undefined, 0, {}, []]) {
  assert.equal(parseSourcepos(value), null, String(value));
}
assert.equal(parseSourcepos("1:1-1:1".repeat(20)), null, "over-long values are refused");
