// What Harper is shown: the source with markup blanked, the filter for its
// lints, and the paragraph windows it is asked to re-check.
import assert from "node:assert/strict";
import { proseText, keepLint, paragraphs } from "../../src/lib/harper/prose.js";

// The masked text has the input's length, so offsets carry over unchanged.
{
  const text = "Some \\emph{words} and $x + y$ here.\nNext $$a$$ line.";
  const { text: masked } = proseText(text, "latex");
  assert.equal(masked.length, text.length);
  assert.ok(masked.includes("words"));
  assert.ok(!masked.includes("$"));
}

// Line breaks inside a skipped span survive, so paragraphs stay apart.
{
  const text = "Before\n\\begin{equation}\nx = y\n\n\\end{equation}\nAfter";
  const { text: masked } = proseText(text, "latex");
  assert.equal(masked.length, text.length);
  assert.equal(masked.split("\n").length, text.split("\n").length);
  assert.ok(!masked.includes("x = y"));
}

// Typst code is blanked and the prose stays.
{
  const text = '#set text(lang: "fr")\nHello there.';
  const { text: masked, skips } = proseText(text, "typst");
  assert.equal(masked.length, text.length);
  assert.ok(masked.endsWith("Hello there."));
  assert.ok(skips.length > 0);
}

// Markdown front matter is blanked and the body stays.
{
  const text = "---\ntitle: My paper\n---\nBody text.";
  const { text: masked } = proseText(text, "markdown");
  assert.equal(masked.length, text.length);
  assert.ok(masked.endsWith("Body text."));
  assert.ok(!masked.includes("title"));
}

// An unknown format is returned as it came.
{
  const { text, skips } = proseText("a $b$ c", "text");
  assert.equal(text, "a $b$ c");
  assert.deepEqual(skips, []);
}

// A lint that overlaps a skip is dropped, one beside it is kept.
{
  const skips = [[10, 20], [30, 40]];
  assert.equal(keepLint({ from: 15, to: 18, problem: "abc" }, skips), false);
  assert.equal(keepLint({ from: 5, to: 12, problem: "abcdefg" }, skips), false);
  assert.equal(keepLint({ from: 18, to: 32, problem: "abcdefghijklmn" }, skips), false);
  assert.equal(keepLint({ from: 0, to: 10, problem: "abc" }, skips), true);
  assert.equal(keepLint({ from: 20, to: 30, problem: "abc" }, skips), true);
  assert.equal(keepLint({ from: 40, to: 45, problem: "abc" }, skips), true);
}

// A lint whose problem text is only whitespace is dropped.
{
  assert.equal(keepLint({ from: 0, to: 3, problem: "   " }, []), false);
  assert.equal(keepLint({ from: 0, to: 3, problem: "\n " }, []), false);
  assert.equal(keepLint({ from: 0, to: 3, problem: "teh" }, []), true);
}

// A position inside a paragraph widens to the paragraph's blank-line edges.
{
  const text = "First one.\nStill first.\n\nSecond one.\n\nThird.";
  assert.deepEqual(paragraphs(text, 14, 14), [{ from: 0, to: 23 }]);
  assert.deepEqual(paragraphs(text, 26, 28), [{ from: 25, to: 36 }]);
}

// A range across two paragraphs returns both, without the blank line between.
{
  const text = "First one.\nStill first.\n\nSecond one.\n\nThird.";
  assert.deepEqual(paragraphs(text, 20, 30), [{ from: 0, to: 23 }, { from: 25, to: 36 }]);
}

// Text with no blank line is one paragraph.
{
  const text = "One line.\nAnother line.";
  assert.deepEqual(paragraphs(text, 3, 12), [{ from: 0, to: text.length }]);
}

// A line holding only whitespace is a blank line.
{
  const text = "A.\n   \nB.";
  assert.deepEqual(paragraphs(text, 0, text.length), [{ from: 0, to: 2 }, { from: 7, to: 9 }]);
}
