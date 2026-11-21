// The project search's matcher, its result shape and its replacement edits.
//
// The panel shows what searchProject returns and applies what replacementsFor
// returns, so these assertions pin the offsets and the context strings the
// editor and the list both depend on.
import assert from "node:assert/strict";
import { compileQuery, replacementsFor, searchProject } from "../../src/lib/project-search.js";

const file = (id, path, text) => ({ id, path, text });
const one = (text, options) => searchProject([file("f", "f.tex", text)], options).groups[0]?.matches ?? [];

// Applies edits sorted by `from` descending, as the panel does.
function applyEdits(text, edits) {
  let out = text;
  for (const e of edits) out = out.slice(0, e.from) + e.insert + out.slice(e.to);
  return out;
}

/* ------------------------------------------------------------- the matcher */

// Regex metacharacters in a plain query are literal.
{
  const text = "cost is $5.00 (approx) and 5x00";
  const matches = one(text, { query: "$5.00 (approx)" });
  assert.equal(matches.length, 1);
  assert.equal(matches[0].from, 8);
  assert.equal(matches[0].to, 22);
  assert.equal(matches[0].text, "$5.00 (approx)");
  assert.equal(one(text, { query: "5.00" }).length, 1, "the dot is not a wildcard");
}

// Case: insensitive by default, sensitive on request.
{
  const text = "foo Foo FOO";
  assert.equal(one(text, { query: "foo" }).length, 3);
  const exact = one(text, { query: "Foo", caseSensitive: true });
  assert.equal(exact.length, 1);
  assert.equal(exact[0].from, 4);
}

// Whole word: a match inside a longer word is skipped, a word before an
// apostrophe still counts.
{
  const text = "cat concatenate cat's cat";
  const froms = one(text, { query: "cat", wholeWord: true }).map((m) => m.from);
  assert.deepEqual(froms, [0, 16, 22]);
}

// Regex mode with groups.
{
  const matches = one("10-20 and 3-4", { query: "(\\d+)-(\\d+)", regex: true });
  assert.deepEqual(matches.map((m) => m.text), ["10-20", "3-4"]);
}

// An invalid regex is reported, never thrown.
{
  assert.deepEqual(compileQuery({ query: "a(", regex: true }), { re: null, error: "invalid" });
  const result = searchProject([file("f", "f.tex", "a(")], { query: "a(", regex: true });
  assert.deepEqual(result, { groups: [], total: 0, capped: false, error: "invalid" });
  // Whole word must not turn a broken pattern into a valid one.
  assert.equal(compileQuery({ query: "a)(", regex: true, wholeWord: true }).error, "invalid");
}

// An empty query matches nothing and is not an error.
assert.deepEqual(compileQuery({ query: "" }), { re: null, error: "" });

// Zero-length matches terminate and are not reported.
{
  const matches = one("baab", { query: "a*", regex: true });
  assert.equal(matches.length, 1);
  assert.equal(matches[0].from, 1);
  assert.equal(matches[0].to, 3);
  assert.equal(one("x\ny", { query: "^", regex: true }).length, 0);
}

// Line and column are 1-based and counted across several lines.
{
  const text = "alpha\nbeta gamma\n  delta gamma";
  const [first, second] = one(text, { query: "gamma" });
  assert.equal(first.line, 2);
  assert.equal(first.column, 6);
  assert.equal(second.line, 3);
  assert.equal(second.column, 9);
}

// Context: 40 characters before, 80 after, with "…" where it was cut.
{
  const text = "x".repeat(50) + "TARGET" + "y".repeat(100);
  const [m] = one(text, { query: "TARGET" });
  assert.equal(m.before, "…" + "x".repeat(40));
  assert.equal(m.after, "y".repeat(80) + "…");
  assert.equal(m.text, "TARGET");
}

// The matched text is cut to 200 characters.
{
  const text = "a".repeat(250);
  const [m] = one(text, { query: "a".repeat(250) });
  assert.equal(m.text, "a".repeat(200) + "…");
}

// A match that spans lines shows only up to the first newline.
{
  const [m] = one("one two\nthree four", { query: "two\nthree" });
  assert.equal(m.text, "two…");
  assert.equal(m.line, 1);
  assert.equal(m.column, 5);
  assert.ok(!m.before.includes("\n") && !m.after.includes("\n"));
}

// Grouping: files with matches only, sorted by path with numeric order.
{
  const files = [
    file("10", "ch10.tex", "hit"),
    file("2", "ch2.tex", "hit hit"),
    file("n", "notes.md", "none"),
  ];
  const result = searchProject(files, { query: "hit" });
  assert.deepEqual(result.groups.map((g) => g.path), ["ch2.tex", "ch10.tex"]);
  assert.deepEqual(result.groups.map((g) => g.id), ["2", "10"]);
  assert.deepEqual(result.groups.map((g) => g.matches.length), [2, 1]);
  assert.equal(result.total, 3);
  assert.equal(result.capped, false);
  assert.equal(result.error, "");
}

// The cap: stops at the limit and says whether more existed.
{
  const five = searchProject([file("f", "f.tex", "a a a a a")], { query: "a" }, { limit: 3 });
  assert.equal(five.total, 3);
  assert.equal(five.capped, true);
  assert.equal(five.groups[0].matches.length, 3);

  const three = searchProject([file("f", "f.tex", "a a a")], { query: "a" }, { limit: 3 });
  assert.equal(three.total, 3);
  assert.equal(three.capped, false);

  const across = searchProject(
    [file("1", "a.tex", "a a"), file("2", "b.tex", "a a")],
    { query: "a" },
    { limit: 3 },
  );
  assert.equal(across.total, 3);
  assert.equal(across.capped, true);
}

/* ------------------------------------------------------------ replacements */

// Literal replacement: `$` in the replacement is not special.
{
  const text = "foo bar foo";
  const options = { query: "foo" };
  const matches = searchProject([file("f", "f.tex", text)], options).groups[0].matches;
  const edits = replacementsFor(text, matches, "$1 & x", options);
  assert.deepEqual(edits, [
    { from: 8, to: 11, insert: "$1 & x" },
    { from: 0, to: 3, insert: "$1 & x" },
  ]);
  assert.equal(applyEdits(text, edits), "$1 & x bar $1 & x");
}

// Regex replacement with $1 and $2, edits in descending order.
{
  const text = "10-20 and 3-4";
  const options = { query: "(\\d+)-(\\d+)", regex: true };
  const matches = searchProject([file("f", "f.tex", text)], options).groups[0].matches;
  const edits = replacementsFor(text, matches, "$2-$1", options);
  assert.deepEqual(edits, [
    { from: 10, to: 13, insert: "4-3" },
    { from: 0, to: 5, insert: "20-10" },
  ]);
  assert.equal(applyEdits(text, edits), "20-10 and 4-3");
}

// Unmatched groups expand to the empty string.
{
  const text = "b";
  const options = { query: "(a)|(b)", regex: true };
  const matches = searchProject([file("f", "f.tex", text)], options).groups[0].matches;
  assert.equal(applyEdits(text, replacementsFor(text, matches, "[$1$2]", options)), "[b]");
}

// Named groups.
{
  const text = "2024-05";
  const options = { query: "(?<y>\\d{4})-(?<m>\\d{2})", regex: true };
  const matches = searchProject([file("f", "f.tex", text)], options).groups[0].matches;
  assert.equal(applyEdits(text, replacementsFor(text, matches, "$<m>/$<y>", options)), "05/2024");
}

// $$ is a dollar sign and $& is the whole match.
{
  const text = "cost 5";
  const options = { query: "\\d", regex: true };
  const matches = searchProject([file("f", "f.tex", text)], options).groups[0].matches;
  assert.equal(applyEdits(text, replacementsFor(text, matches, "$$$&", options)), "cost $5");
  assert.equal(applyEdits(text, replacementsFor(text, matches, "$&$&", options)), "cost 55");
}

// Lookahead: the bar after the match is part of the test, but not replaced.
{
  const text = "foo bar foo baz";
  const options = { query: "foo(?= bar)", regex: true };
  const matches = searchProject([file("f", "f.tex", text)], options).groups[0].matches;
  const edits = replacementsFor(text, matches, "qux", options);
  assert.deepEqual(edits, [{ from: 0, to: 3, insert: "qux" }]);
  assert.equal(applyEdits(text, edits), "qux bar foo baz");
}

// A match whose text changed since the search is skipped.
{
  const options = { query: "foo" };
  const matches = searchProject([file("f", "f.tex", "foo bar foo")], options).groups[0].matches;
  const edits = replacementsFor("foo bar fox", matches, "X", options);
  assert.deepEqual(edits, [{ from: 0, to: 3, insert: "X" }]);
}
