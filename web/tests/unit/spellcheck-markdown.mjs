// Checks markdownSkips on Markdown and Quarto source: which ranges are markup
// the spellchecker must skip, and which words it still sees. Each case states
// the words that should remain, so a missed skip and an over-eager skip both
// fail. Ranges must stay in bounds, and hostile input must not throw.

import assert from "node:assert/strict";

const { markdownSkips } = await import("../../src/lib/spellcheck/markdown.js");

// The words the spellchecker still sees: the text with every skipped range
// blanked, split on anything that is not a letter.
function words(text) {
  const kept = text.split("");
  for (const [from, to] of markdownSkips(text)) for (let i = from; i < to; i++) kept[i] = " ";
  return kept.join("").split(/[^\p{L}]+/u).filter(Boolean);
}

// Plain prose has nothing to skip.
{
  assert.deepEqual(markdownSkips("Plain words here."), []);
  assert.deepEqual(words("Plain words here."), ["Plain", "words", "here"]);
}

// A Pandoc attribute at the end of a heading is markup; the heading text is prose.
{
  assert.deepEqual(words("# Heading {#sec-intro}\nBody"), ["Heading", "Body"]);
}

// Emphasis markers are not words, but the words inside them are.
{
  assert.deepEqual(words("**bold** and *it*"), ["bold", "and", "it"]);
}

// YAML front matter is skipped whole, keys and values alike.
{
  assert.deepEqual(words("---\ntitle: My paper\nformat: html\n---\nBody"), ["Body"]);
}

// A Quarto executable cell is a fence, so its option lines and code are skipped.
{
  assert.deepEqual(words("```{r}\n#| echo: false\nx <- 1\n```\nAfter"), ["After"]);
}

// A tilde fence is skipped the same way as a backtick fence.
{
  assert.deepEqual(words("~~~\nhidden code\n~~~\nShown"), ["Shown"]);
}

// Inline math with a non-space after the opener and before the closer is skipped.
{
  assert.deepEqual(words("The term $x_i$ is set."), ["The", "term", "is", "set"]);
}

// Dollar amounts are not math: the closer needs a non-space before it.
{
  assert.deepEqual(words("costs $5 and $10"), ["costs", "and"]);
}

// Display math may span lines and is skipped whole.
{
  assert.deepEqual(words("$$\n\\sum\n$$\nAfter"), ["After"]);
}

// Inline code is skipped up to its closing run of backticks.
{
  assert.deepEqual(words("Use `foo <- bar` here."), ["Use", "here"]);
}

// A link target and its title are markup; the link text is prose.
{
  assert.deepEqual(words('[link text](https://example.org/path "Title") ends.'), ["link", "text", "ends"]);
}

// An image target and its attributes are markup; the alt text is prose.
{
  assert.deepEqual(words("![alt words](figs/plot.png){width=50%} then"), ["alt", "words", "then"]);
}

// Citation keys are markup; the locator after them is prose.
{
  assert.deepEqual(words("[@smith2020; @doe2019, p. 3]"), ["p"]);
}

// A narrative citation skips only the key.
{
  assert.deepEqual(words("As @smith2020 shows."), ["As", "shows"]);
}

// A Quarto cross reference is a citation key; trailing punctuation is prose.
{
  assert.deepEqual(words("See @fig-plot and @sec-intro."), ["See", "and"]);
}

// An e-mail address is skipped whole, so its @ is not read as a citation.
{
  assert.deepEqual(words("mail me at someone@example.org"), ["mail", "me", "at"]);
}

// Div markers and the class name after them are markup; the div body is prose.
{
  assert.deepEqual(
    words("::: {.callout-note}\nNote text\n:::\n::: callout-tip\nTip text"),
    ["Note", "text", "Tip", "text"],
  );
}

// A Quarto shortcode is skipped whole.
{
  assert.deepEqual(words("{{< pagebreak >}}\nNext"), ["Next"]);
}

// Raw HTML tags are skipped with their attributes; the text between them is prose.
{
  assert.deepEqual(words('<span class="x">word</span>'), ["word"]);
}

// An HTML comment is skipped whole.
{
  assert.deepEqual(words("<!-- hidden note -->\nVisible"), ["Visible"]);
}

// A footnote marker is skipped, and a definition keeps its note text as prose.
{
  assert.deepEqual(words("text[^fn1] more\n\n[^fn1]: The note."), ["text", "more", "The", "note"]);
}

// A reference definition line is skipped, and a reference link keeps its text.
{
  assert.deepEqual(words('[id]: https://example.org "Title"\nSee [the paper][id] now.'), [
    "See",
    "the",
    "paper",
    "now",
  ]);
}

// An unterminated fence runs to the end of the text.
{
  assert.deepEqual(words("Before\n```\nnever closed\nstill code"), ["Before"]);
}

// An unterminated front matter block runs to the end of the text.
{
  assert.deepEqual(words("---\ntitle: Never closed\nBody"), []);
}

// An unterminated display block runs to the end of the text.
{
  assert.deepEqual(words("Before\n$$\nx\nAfter"), ["Before"]);
}

// A backslash escape covers the escaped dollar sign, so it cannot open math.
{
  assert.deepEqual(words("Escaped \\$x\\$ dollars"), ["Escaped", "x", "dollars"]);
}

// A bare URL runs up to whitespace and is skipped.
{
  assert.deepEqual(words("See https://example.org/a?b=c for more."), ["See", "for", "more"]);
}

// An autolink in angle brackets is skipped whole.
{
  assert.deepEqual(words("Visit <https://example.org> now"), ["Visit", "now"]);
}

// Class attributes at the end of a heading are markup, like an ID.
{
  assert.deepEqual(words("## Intro {.unnumbered}"), ["Intro"]);
}

// An empty document has no skips.
{
  assert.deepEqual(markdownSkips(""), []);
}

// Every returned range is a valid half-open range inside the text.
{
  const samples = [
    "---\na: b\n---\n# T {#x}\n```{r}\n#| e\n```\n$$\nq\n$$ and $x$ @k [t](u){v} <a>b</a> <!-- c -->",
    "text[^a] [^a]: n\n:::: {.x}\n\n$$",
    "[id]: u\n[a][b] <https://x.y> https://z.w)",
  ];
  for (const sample of samples) {
    for (const [from, to] of markdownSkips(sample)) {
      assert.ok(Number.isInteger(from) && Number.isInteger(to), "range bounds are integers");
      assert.ok(0 <= from && from <= to && to <= sample.length, "range lies inside the text");
    }
  }
}

// Hostile and partial input never throws, and always yields valid ranges.
{
  const hostile = [
    "[", "]", "(", ")", "`", "``", "$", "$$", "$$$", "{", "{{<", "}", "<", "<!--", "<!",
    ":::", "```", "~~~", "\\", "@", "h", "http", "https://", "[^", "---", "#", "|", "\n\n\n",
    "[](", "![](", "`a ", "$a ", "@@", "a\r\nb", "\u0000", "\uD83D",
  ];
  for (const sample of hostile) {
    const ranges = markdownSkips(sample);
    assert.ok(Array.isArray(ranges), `returns an array for ${JSON.stringify(sample)}`);
    for (const [from, to] of ranges) assert.ok(0 <= from && from <= to && to <= sample.length);
  }
  assert.deepEqual(markdownSkips(undefined), []);
}

// Long lines full of unmatched openers finish quickly instead of rescanning.
{
  const long = [
    "[".repeat(20000),
    "`a ".repeat(5000),
    "$a ".repeat(5000),
    "<!--".repeat(5000),
    "{{<".repeat(5000),
    "@a".repeat(5000),
  ];
  for (const sample of long) assert.ok(Array.isArray(markdownSkips(sample)));
}

// A long line of unmatched footnote openers keeps the word after it and does not hang.
{
  assert.deepEqual(words("[^".repeat(20000) + " word"), ["word"]);
}

// A long line of unmatched tag openers keeps the word after it and does not hang.
{
  assert.ok(words("<a".repeat(20000) + " word").includes("word"));
}

// A long line of unmatched braces keeps the word after it and does not hang.
{
  assert.ok(words("{".repeat(20000) + " word").includes("word"));
}

// A long line of unmatched link openers keeps the word after it and does not hang.
{
  assert.ok(words("[x".repeat(20000) + " word").includes("word"));
}

// A long line of unmatched citation markers keeps the word after it and does not hang.
{
  assert.ok(words("@".repeat(20000) + " word").includes("word"));
}

// Inline code inside a link label is still skipped; the label text stays prose.
{
  assert.deepEqual(words("[`identifier` here](https://x.org)"), ["here"]);
}

// Inline math inside a link label is still skipped; the rest of the label stays prose.
{
  assert.deepEqual(words("[$alpha$ and more](x.html)"), ["and", "more"]);
}

// A citation inside a link label is still skipped; the words before it stay prose.
{
  assert.deepEqual(words("[see @smith2020](x)"), ["see"]);
}

// Many link openers sharing one closing bracket with a failing attribute run
// finish, return few ranges, and keep the word after them.
{
  const sample = "[".repeat(20000) + "]{" + "a".repeat(20000) + " word";
  assert.ok(markdownSkips(sample).length < 100);
  assert.equal(words(sample).at(-1), "word");
}

// Many link openers sharing one closing bracket with an unclosed target
// finish and skip the target as one bounded range.
{
  const sample = "[".repeat(20000) + "](" + "a".repeat(20000);
  assert.ok(markdownSkips(sample).length < 100);
  assert.deepEqual(words(sample), []);
}

// Many unmatched link openers keep the word after them.
{
  assert.deepEqual(words("[".repeat(20000) + " word"), ["word"]);
}
