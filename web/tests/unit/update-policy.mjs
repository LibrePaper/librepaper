// When a reader is moved to a newer published version, and how they are put
// back where they were reading.
import test from "node:test";
import assert from "node:assert/strict";
import { bookmarkFrom, mayApplyUpdate } from "../../src/lib/reader/update-policy.js";
import { anchorOne, flatten } from "../../src/lib/anchor.js";

test("update-policy: an offer is taken when the reader is doing nothing with the page", () => {
  assert.equal(mayApplyUpdate({ offered: true }), true);
  assert.equal(mayApplyUpdate({ offered: false }), false);
  assert.equal(mayApplyUpdate({}), false);
});

test("update-policy: a held selection or an open draft holds the update back", () => {
  assert.equal(mayApplyUpdate({ offered: true, selecting: true }), false);
  assert.equal(mayApplyUpdate({ offered: true, composing: true }), false);
  // And releasing it is what lets the update through, without a second offer:
  // the effect that calls this re-runs when either clears.
  assert.equal(mayApplyUpdate({ offered: true, selecting: false, composing: false }), true);
});

test("update-policy: a bookmark starts at a whole word", () => {
  const text = "The interval covers the mean of the posterior distribution, which is what we report.";
  // An offset landing inside "interval" backs up to where the word began.
  const bookmark = bookmarkFrom(text, 7);
  assert.ok(bookmark.exact.startsWith("interval covers"), bookmark.exact);
});

test("update-policy: there is no bookmark for an empty page or a scrap of one", () => {
  assert.equal(bookmarkFrom("", 0), null);
  assert.equal(bookmarkFrom(null, 0), null);
  assert.equal(bookmarkFrom("short", 0), null);
  // An offset past the end is a position, not a crash.
  assert.equal(bookmarkFrom("short", 9000), null);
});

test("update-policy: the reader comes back to the same words after the page grows above them", () => {
  const before = [
    "# A paper",
    "The first section says one thing.",
    "The interval covers the mean of the posterior, which is the result this paper reports.",
    "A closing paragraph.",
  ].join("\n\n");
  const at = before.indexOf("The interval covers");
  const bookmark = bookmarkFrom(before, at);
  assert.ok(bookmark, "there is something to come back to");

  // The next published version has a whole section inserted above the reader,
  // so every offset they had is wrong and the words are not.
  const after = before.replace(
    "The first section says one thing.",
    "The first section says one thing.\n\nAn entirely new paragraph, added while they were reading.",
  );
  const found = anchorOne(after, { ...bookmark, position: null, requireUnique: true }, flatten(after));
  assert.ok(found, "the passage is found again");
  assert.equal(after.slice(found.start, found.start + 19), "The interval covers");
  assert.notEqual(found.start, at, "and it is not where it was");
});

test("update-policy: a passage rewritten out of the new version is simply not found", () => {
  const before = "The interval covers the mean of the posterior, which is what this paper reports today.";
  const bookmark = bookmarkFrom(before, 0);
  const after = "Everything here was rewritten between one published version and the next one.";
  const found = anchorOne(after, { ...bookmark, position: null, requireUnique: true }, flatten(after));
  assert.equal(found, null, "nothing is found, so the page opens where it opens");
});
