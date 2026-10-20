// The matching behind in-page search, without a browser.

import assert from "node:assert/strict";
import { MAX_MATCHES, findOffsets, normalize, pickIndex } from "../../src/lib/pdf/find.js";

const slices = (text, query) => findOffsets(text, query).map(({ start, end }) => text.slice(start, end));

assert.deepEqual(slices("Hello hello HELLO", "hello"), ["Hello", "hello", "HELLO"]);
assert.deepEqual(findOffsets("abc abc", "bc"), [
  { start: 1, end: 3 },
  { start: 5, end: 7 },
]);

// Whitespace in the text, a gap's newline included, matches any run in the query.
assert.deepEqual(slices("the  quick\n\nbrown fox", "quick brown"), ["quick\n\nbrown"]);
assert.deepEqual(slices("the quick brown", "quick   brown"), ["quick brown"]);
assert.deepEqual(slices("a b", "  a b  "), ["a b"], "the query is trimmed");

// A line break does not let a match join words that are not adjacent.
assert.deepEqual(findOffsets("end\nstart", "endstart"), []);

assert.deepEqual(findOffsets("anything", ""), []);
assert.deepEqual(findOffsets("anything", "   "), []);
assert.deepEqual(findOffsets("aaaa", "aa"), [
  { start: 0, end: 2 },
  { start: 2, end: 4 },
], "matches do not overlap");

// Folding keeps lengths, so offsets stay honest around characters whose lower
// case is longer (U+0130 becomes two code units in some locales).
const odd = "İx X";
assert.equal(normalize(odd).norm.length, odd.length);
assert.deepEqual(slices(odd, "x"), ["x", "X"]);

assert.equal(findOffsets("a".repeat(50), "a", 7).length, 7, "the limit holds");
assert.equal(MAX_MATCHES, 10000);

// Choosing the match to land on.
const pages = [1, 1, 3, 3, 5];
assert.equal(pickIndex(pages, -1, 0, 1), 0);
assert.equal(pickIndex(pages, -1, 0, 2), 2, "first on or after the page");
assert.equal(pickIndex(pages, -1, 0, 4), 4);
assert.equal(pickIndex(pages, -1, 0, 9), 0, "wraps to the first");
assert.equal(pickIndex(pages, 4, 1, 1), 0, "next wraps");
assert.equal(pickIndex(pages, 0, -1, 1), 4, "previous wraps");
assert.equal(pickIndex(pages, 1, 1, 1), 2);
assert.equal(pickIndex(pages, -1, -1, 1), 4, "previous with no current starts at the last");
assert.equal(pickIndex([], 0, 1, 1), -1);
