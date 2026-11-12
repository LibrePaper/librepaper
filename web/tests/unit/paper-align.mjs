import assert from "node:assert/strict";
import { alignText, mapPoint, mapRange, normalizeSpace } from "../../src/lib/paper/align.js";

// The map of a source against what its rendering shows, with whitespace normalized the way the painter does.
const align = (source, rendered) => ({
  source: normalizeSpace(source),
  map: alignText(normalizeSpace(source), normalizeSpace(rendered)),
});
const range = (aligned, from, to) => mapRange(aligned.map, aligned.source, from, to);

// Identical texts align one to one.
{
  const { map } = align("plain text", "plain text");
  assert.deepEqual(Array.from(map), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

// Emphasis: the words align, the asterisks do not, and a range over the
// markup too lands on the words.
{
  const aligned = align("Some *emph* word", "Some emph word");
  assert.deepEqual(range(aligned, 6, 10), { from: 5, to: 9 });
  assert.deepEqual(range(aligned, 5, 11), { from: 5, to: 9 });
  assert.equal(range(aligned, 5, 6), null);
}

// A heading: a mark over the marker alone is unmapped, over the line it lands on the title.
{
  const aligned = align("# Title", "Title");
  assert.equal(range(aligned, 0, 2), null);
  assert.deepEqual(range(aligned, 0, 7), { from: 0, to: 5 });
}

// A link: the label aligns, the target is markup.
{
  const source = "see [the docs](https://x.org/a) now";
  const aligned = align(source, "see the docs now");
  assert.deepEqual(range(aligned, 5, 13), { from: 4, to: 12 });
  const url = source.indexOf("https");
  assert.equal(range(aligned, url, url + "https://x.org/a".length), null);
}

// A list: bullets are markup and the rendered text has its own line breaks.
{
  const aligned = align("- one\n- two", "\none\n\ntwo\n");
  assert.deepEqual(range(aligned, 8, 11), { from: 6, to: 9 });
}

// A formula is one unit in the rendered text, standing for its TeX; the
// dollar signs are source-only.
{
  const aligned = align("Let $x^2$ be", "Let x^2 be");
  assert.deepEqual(range(aligned, 4, 9), { from: 4, to: 7 });
  assert.equal(range(aligned, 4, 5), null);
}

// A point lands before the next aligned character, or after the last one.
{
  const { map } = align("a *b* c", "a b c");
  assert.equal(mapPoint(map, 3), 2);
  assert.equal(mapPoint(map, 4), 3);
  assert.equal(mapPoint(map, 7), 5);
  assert.equal(mapPoint(new Int32Array([-1, -1]), 1), null);
}

// Texts that are too long, or too different, are not aligned.
{
  assert.equal(alignText("a".repeat(15000), "b".repeat(6000)), null);
  assert.equal(alignText("abcdef", "uvwxyz", { maxEdits: 3 }), null);
}

// Whitespace is compared as one space each, so a line break matches a space.
{
  const aligned = align("one\ntwo", "one two");
  assert.deepEqual(range(aligned, 0, 7), { from: 0, to: 7 });
}

console.log("paper-align: ok");
