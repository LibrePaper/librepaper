// The parts of a source the spellchecker skips.
//
// Each format's scanner is tested on its own. What is checked here is what the
// dispatcher adds: unknown formats get nothing, the output is sorted and
// disjoint with touching ranges merged, every range is non-empty and inside
// the text, and Quarto shares the Markdown scanner.
import assert from "node:assert/strict";
import { skipRanges } from "../../src/lib/spellcheck/index.js";

// The shape every result must have, whatever the format or text.
function assertWellFormed(ranges, text) {
  for (const [from, to] of ranges) {
    assert.ok(Number.isInteger(from) && Number.isInteger(to), "offsets are integers");
    assert.ok(to > from, "ranges are non-empty");
    assert.ok(from >= 0 && to <= text.length, "ranges stay inside the text");
  }
  for (let i = 1; i < ranges.length; i++) {
    // Strictly after the previous range: touching ranges would have been merged.
    assert.ok(ranges[i][0] > ranges[i - 1][1], "ranges are sorted, disjoint and not touching");
  }
}

// An unknown format has no scanner, so nothing is skipped.
{
  assert.deepEqual(skipRanges("$\\alpha$ and \\cite{a}", "text"), []);
}

// The empty format, which is what a file with no known extension gets.
{
  assert.deepEqual(skipRanges("# Title", ""), []);
}

// A LaTeX line with math and a citation: overlapping constructs come out sorted and disjoint.
{
  const text = "$\\alpha$ and \\cite{a}\n\\begin{equation} x^2 \\end{equation} and \\textbf{bold} prose.";
  const ranges = skipRanges(text, "latex");
  assertWellFormed(ranges, text);
}

// Typst source with code and markup together, checked for the same invariants.
{
  const text = "#let x = 1\n$ sum_(i=0)^n i $ and some *prose* here, @smith2020.";
  const ranges = skipRanges(text, "typst");
  assertWellFormed(ranges, text);
}

// HTML with tags and a script body, checked for the same invariants.
{
  const text = "<p>Some <em>prose</em> and <script>var x = 1;</script> more.</p>";
  const ranges = skipRanges(text, "html");
  assertWellFormed(ranges, text);
}

// Quarto and Markdown share one scanner, so the same text gives the same ranges.
{
  const text = "# Heading\n\nSome `code` and $x^2$ with [a link](https://example.org).\n\n```python\nprint(1)\n```\n";
  assert.deepEqual(skipRanges(text, "quarto"), skipRanges(text, "markdown"));
  assertWellFormed(skipRanges(text, "markdown"), text);
}

// An empty text gives no ranges in any known format.
{
  for (const format of ["latex", "typst", "markdown", "quarto", "html"]) {
    assert.deepEqual(skipRanges("", format), [], `${format} on empty text`);
  }
}
