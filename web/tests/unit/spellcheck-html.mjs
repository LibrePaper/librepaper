// Checks htmlSkips, the scanner that tells the spellchecker which ranges of an
// HTML source are markup. Tags, comments, raw elements, entities, TeX math and
// bare URLs must be skipped; attribute text for alt and title and all text
// between tags must be kept. Also checks that ranges stay in bounds and that
// malformed input does not throw.

import assert from "node:assert/strict";

const { htmlSkips } = await import("../../src/lib/spellcheck/html.js");

// The words the spellchecker still sees: the text with every skipped range
// blanked, split on anything that is not a letter.
function words(text) {
  const kept = text.split("");
  for (const [from, to] of htmlSkips(text)) for (let i = from; i < to; i++) kept[i] = " ";
  return kept.join("").split(/[^\p{L}]+/u).filter(Boolean);
}

// Plain text has no markup, so every word is kept.
{
  assert.deepEqual(words("Plain text here"), ["Plain", "text", "here"]);
}

// The empty string has no ranges.
{
  assert.deepEqual(htmlSkips(""), []);
}

// A class attribute is markup; the text between the tags is prose.
{
  assert.deepEqual(words('<p class="lead">Hello world</p>'), ["Hello", "world"]);
}

// The href URL is markup; the link text is prose.
{
  assert.deepEqual(words('<a href="https://x.org/a?b=c">link text</a>'), ["link", "text"]);
}

// The alt value is prose, but the src path is markup.
{
  assert.deepEqual(words('<img src="figs/a.png" alt="A tidy chart">'), ["A", "tidy", "chart"]);
}

// The title value is prose, so all four words of the abbreviation are kept.
{
  assert.deepEqual(
    words('<abbr title="World Health Organization">WHO</abbr>'),
    ["World", "Health", "Organization", "WHO"],
  );
}

// A quoted > inside an attribute does not end the tag early.
{
  assert.deepEqual(words('<span data-x="a > b">Kept</span>'), ["Kept"]);
}

// Comments and doctype declarations are markup.
{
  assert.deepEqual(words("<!-- note --><!DOCTYPE html><p>Body</p>"), ["Body"]);
}

// Script and style bodies are code, not prose.
{
  assert.deepEqual(
    words("<script>const xyzzy = 1;</script><style>.foo{}</style><p>Kept</p>"),
    ["Kept"],
  );
}

// Pre and code bodies are skipped, and the prose around them is kept.
{
  assert.deepEqual(words("<pre>int plugh;</pre><p>Text <code>quux()</code> after</p>"), ["Text", "after"]);
}

// Kbd, samp, math and svg bodies are skipped as well.
{
  assert.deepEqual(
    words("<kbd>Ctrl</kbd> <samp>out</samp> <math>abc</math> <svg>defs</svg> end"),
    ["end"],
  );
}

// Named and numeric character references are skipped; the words around them stay.
{
  assert.deepEqual(words("caf&eacute; &amp; more"), ["caf", "more"]);
}

// A TeX inline math span is skipped.
{
  assert.deepEqual(words("Let \\(x^2\\) be small"), ["Let", "be", "small"]);
}

// A TeX display math span is skipped.
{
  assert.deepEqual(words("Then \\[y\\] holds"), ["Then", "holds"]);
}

// Dollar signs are not math delimiters in HTML, so the dollar text is prose.
{
  assert.deepEqual(words("Cost $x$ here"), ["Cost", "x", "here"]);
}

// A bare URL in text is skipped up to whitespace.
{
  assert.deepEqual(words("See https://example.org/a/b now"), ["See", "now"]);
}

// A less-than sign followed by a space does not open a tag.
{
  assert.deepEqual(words("a < b"), ["a", "b"]);
}

// Tag names match case-insensitively, so an uppercase script is skipped.
{
  assert.deepEqual(words("<SCRIPT>const xyzzy;</SCRIPT>after"), ["after"]);
}

// An uppercase end tag with a space before its > closes the element.
{
  assert.deepEqual(words("<SCRIPT>x</Script >after"), ["after"]);
}

// A longer name that starts like the end tag is not an end tag.
{
  assert.deepEqual(words("<style>a</style-x></styles>b</style>c"), ["c"]);
}

// Many end-tag fragments with no closing > must not make the scan quadratic.
// The element runs to the end of the text, so nothing after it is kept.
{
  const hostile = `<script>${"</script ".repeat(20000)} word`;
  assert.deepEqual(words(hostile), []);
  const ranges = htmlSkips(hostile);
  assert.equal(ranges[ranges.length - 1][1], hostile.length);
}

// An unterminated tag runs to the end of the text and does not throw.
{
  assert.deepEqual(words('<div class="x'), []);
  assert.deepEqual(words('Hello <div class="x'), ["Hello"]);
}

// An unterminated comment runs to the end of the text and does not throw.
{
  assert.deepEqual(words("Keep <!-- never ends"), ["Keep"]);
}

// Every returned range is an in-bounds, ordered pair of offsets.
{
  const samples = [
    "",
    "<",
    "&",
    "\\(",
    "https://",
    '<img alt="x" title=\'y\'>',
    "<script>",
    "<!--",
    "<![CDATA[",
    "<?php",
    "&#x1F; &#123; &name;",
    "<p class=\"lead\">Hello world</p>",
  ];
  for (const sample of samples) {
    for (const [from, to] of htmlSkips(sample)) {
      assert.ok(Number.isInteger(from) && Number.isInteger(to), sample);
      assert.ok(0 <= from && from <= to && to <= sample.length, sample);
    }
  }
}
