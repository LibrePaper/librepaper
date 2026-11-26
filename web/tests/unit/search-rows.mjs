// The results list's row grouping and path split. Matches come from
// searchProject, so the shapes are the ones the panel really gets.
import assert from "node:assert/strict";
import { lineRows, splitPath } from "../../src/lib/search-rows.js";
import { searchProject } from "../../src/lib/project-search.js";

const matchesOf = (text, query, options = {}) =>
  searchProject([{ id: "f", path: "f.md", text }], { query, ...options }).groups[0]?.matches ?? [];

// Two matches on one line share a row, with the gap between them as context.
{
  const matches = matchesOf("cat dog cat!", "cat");
  const rows = lineRows(matches);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].line, 1);
  assert.equal(rows[0].matches.length, 2);
  assert.equal(rows[0].before, "");
  assert.deepEqual(rows[0].segments, [
    { text: "cat", mark: true },
    { text: " dog ", mark: false },
    { text: "cat", mark: true },
    { text: "!", mark: false },
  ]);
}

// Three matches on one line are one row with three highlights, and adjacent
// matches with no gap add no empty segment.
{
  const rows = lineRows(matchesOf("a-a-a", "a"));
  assert.equal(rows.length, 1);
  assert.equal(rows[0].matches.length, 3);
  assert.deepEqual(rows[0].segments.map((s) => s.text), ["a", "-", "a", "-", "a"]);
  const adjacent = lineRows(matchesOf("aaa", "a"));
  assert.deepEqual(adjacent[0].segments, [{ text: "a", mark: true }, { text: "a", mark: true }, { text: "a", mark: true }]);
}

// A gap longer than the first match's after-context starts a new row.
{
  const text = "foo " + "x".repeat(100) + " foo";
  const rows = lineRows(matchesOf(text, "foo"));
  assert.equal(rows.length, 2);
  assert.equal(rows[1].before, "…" + "x".repeat(39) + " ");
  assert.deepEqual(rows[0].segments, [
    { text: "foo", mark: true },
    { text: " " + "x".repeat(79) + "…", mark: false },
  ]);
}

// Matches on different lines are different rows.
{
  const rows = lineRows(matchesOf("cat\ncat", "cat"));
  assert.deepEqual(rows.map((r) => r.line), [1, 2]);
  assert.equal(rows[0].matches.length, 1);
  assert.equal(rows[1].matches.length, 1);
}

// A truncated match does not merge with the match after it, even when the
// gap is covered by its after-context.
{
  const long = {
    from: 0, to: 250, line: 1, column: 1, before: "",
    text: "a".repeat(200) + "…", after: " cat",
  };
  const next = { from: 251, to: 254, line: 1, column: 252, before: "", text: "cat", after: "" };
  const rows = lineRows([long, next]);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows[0].segments, [{ text: long.text, mark: true }, { text: " cat", mark: false }]);
}

// Empty input has no rows.
assert.deepEqual(lineRows([]), []);

// Paths split at the last slash; a bare name has no directory.
assert.deepEqual(splitPath("chapters/intro.md"), { name: "intro.md", dir: "chapters" });
assert.deepEqual(splitPath("a.md"), { name: "a.md", dir: "" });
assert.deepEqual(splitPath("a/b/c.tex"), { name: "c.tex", dir: "a/b" });
