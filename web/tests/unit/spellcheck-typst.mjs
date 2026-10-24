// Checks the Typst spellcheck scanner: which ranges of a source are markup the
// spellchecker must skip, and which words it still reads as prose.
import assert from "node:assert/strict";

const { typstSkips } = await import("../../src/lib/spellcheck/typst.js");

// The words the spellchecker still sees: the text with every skipped range
// blanked, split on anything that is not a letter.
function words(text) {
  const kept = text.split("");
  for (const [from, to] of typstSkips(text)) for (let i = from; i < to; i++) kept[i] = " ";
  return kept.join("").split(/[^\p{L}]+/u).filter(Boolean);
}

// Plain prose has no markup, so nothing is skipped.
{
  assert.deepEqual(typstSkips("Plain prose, with words."), []);
}

// A heading marker is markup, but the heading words are prose.
{
  assert.deepEqual(words("= Introduction\n\nBody text."), ["Introduction", "Body", "text"]);
}

// Emphasis and italic delimiters are markup; the words inside stay prose.
{
  assert.deepEqual(words("*bold* and _it_"), ["bold", "and", "it"]);
}

// A set rule is code to the end of its line.
{
  assert.deepEqual(words('#set text(lang: "fr", font: "Libertinus Serif")'), []);
}

// A let binding is code to the end of its line.
{
  assert.deepEqual(words('#let title = "Draft"'), []);
}

// An import names a package, which is code.
{
  assert.deepEqual(words('#import "@preview/cetz:0.2.2": canvas'), []);
}

// Call arguments are code, the caption content block is prose, and the label is markup.
{
  const src = '#figure(image("plot.png", width: 80%), caption: [A noisy measurement]) <fig:plot>';
  assert.deepEqual(words(src), ["A", "noisy", "measurement"]);
}

// A trailing content block is prose, and the call name is code.
{
  assert.deepEqual(words("#emph[strong words]"), ["strong", "words"]);
}

// Inline math, delimiters included, is skipped.
{
  assert.deepEqual(typstSkips("$x^2 + alpha$"), [[0, 13]]);
  assert.deepEqual(words("$x^2 + alpha$"), []);
}

// Math may span lines, so its words are skipped too.
{
  assert.deepEqual(words("Let $ alpha\n + beta $ hold."), ["Let", "hold"]);
}

// Unclosed math ends at its line end, so the next line is prose.
{
  assert.deepEqual(words("Unclosed $x\nmore"), ["Unclosed", "more"]);
}

// A backslash-escaped dollar is a literal dollar, not an opening of math.
{
  assert.deepEqual(typstSkips("\\$5"), [[0, 2]]);
  assert.deepEqual(words("Costs \\$5 now"), ["Costs", "now"]);
}

// A reference is skipped, but its trailing period is punctuation.
{
  assert.deepEqual(typstSkips("See @smith2020."), [[4, 14]]);
  assert.deepEqual(words("See @smith2020."), ["See"]);
}

// A line comment runs to the end of its line.
{
  assert.deepEqual(typstSkips("Text // note\nMore"), [[5, 12]]);
}

// A block comment may span lines, and its words are skipped.
{
  assert.deepEqual(words("A /* block \n comment */ B"), ["A", "B"]);
}

// Inline raw text is skipped.
{
  assert.deepEqual(words("Use `code here` now"), ["Use", "now"]);
}

// A fenced raw block is skipped with its language tag and body.
{
  assert.deepEqual(words("```python\nprint(x)\n```\nAfter"), ["After"]);
}

// Unclosed inline raw ends at its line end.
{
  assert.deepEqual(words("a `unclosed\nnext"), ["a", "next"]);
}

// Unclosed fenced raw runs to the end of the text.
{
  assert.deepEqual(words("```\nlet x\nmore"), []);
}

// Unclosed block comment runs to the end of the text.
{
  assert.deepEqual(words("A /* open\nB"), ["A"]);
}

// A dotted symbol chain after the hash is code.
{
  assert.deepEqual(words("Go #sym.arrow.r now"), ["Go", "now"]);
}

// A URL runs to the next whitespace and is skipped.
{
  assert.deepEqual(words("See https://example.org/a?b=c now"), ["See", "now"]);
}

// Content blocks nest: the innermost words are still prose.
{
  assert.deepEqual(words("#strong[#emph[deep word]]"), ["deep", "word"]);
}

// A content block in a let line is prose, and the next line is prose too.
{
  assert.deepEqual(words("#let note = [Inside here]\nNext"), ["Inside", "here", "Next"]);
}

// An unbalanced call does not throw, and the prose after it is still read.
{
  assert.deepEqual(words("#f(\nHello\nWorld"), ["Hello", "World"]);
}

// A multi-line call that closes is skipped as a whole.
{
  assert.deepEqual(words("#f(\n  a: 1\n)\nDone"), ["Done"]);
}

// A multi-line #set call keeps its argument lines as code, and the next line is prose.
{
  assert.deepEqual(
    words('#set text(\n  font: "Libertinus Serif",\n  lang: "en",\n)\nBody words here.'),
    ["Body", "words", "here"],
  );
}

// A multi-line #show rule with an open call on its first line ends at the line after the call.
{
  assert.deepEqual(words('#show heading: set text(\n  weight: "bold",\n)\nAfter text'), ["After", "text"]);
}

// A multi-line #let function body in braces is code, so the whole definition is skipped.
{
  assert.deepEqual(words("#let f(x) = {\n  x + 1\n}"), []);
}

// A content block under a #show line is prose, and the field access inside it is code.
{
  assert.deepEqual(words("#show heading: it => [\n  #it.body words\n]"), ["words"]);
}

// The Insert menu's cross-reference code is code, and the link text in its content block is prose.
{
  assert.deepEqual(
    words(
      '#context { let target = query(<fig:plot>).first(); if target.has("numbering") and target.numbering != none { ref(<fig:plot>) } else { link(<fig:plot>)[the plot] } }',
    ),
    ["the", "plot"],
  );
}

// A for loop's header is code, and its content block is prose, with the text after the block as markup.
{
  assert.deepEqual(words("#for x in range(3) [item #x ] and more"), ["item", "and", "more"]);
}

// An if with an else branch: both content blocks are prose, and the else stays on the if line.
{
  assert.deepEqual(words("#if x [yes] else [no]"), ["yes", "no"]);
}

// An else after a closing brace on the same line keeps its content block as prose, and the next line is prose.
{
  assert.deepEqual(
    words("#if cond {\n  let x = 1\n} else [\n  Fallback wordz here\n]\nAfter"),
    ["Fallback", "wordz", "here", "After"],
  );
}

// A context call with a code expression has no prose.
{
  assert.deepEqual(words("#context text.lang"), []);
}

// A semicolon ends a statement, and the prose after it is checked. One inside
// a group or a string does not.
{
  assert.deepEqual(words("#let x = 1; Misspeled sentence."), ["Misspeled", "sentence"]);
  assert.deepEqual(words("#let f(a) = { a; qwrtz }\nNext"), ["Next"]);
  assert.deepEqual(words("#let s = \"a; b\"\nNext"), ["Next"]);
}

// A quote inside a string argument does not close the string early.
{
  assert.deepEqual(words('#f("a\\"b)") x'), ["x"]);
}

// A string that is never closed ends at its line, and the next line is prose.
{
  assert.deepEqual(words('#f("abc\nnext'), ["next"]);
}

// Empty input has nothing to skip.
{
  assert.deepEqual(typstSkips(""), []);
}

// Every range is a valid half-open slice of its text, even for broken input.
{
  const samples = ["", "#f(", "$x", "`", "/*", "a\\", "#let x = [", "@", "<a", "https://", "]]}}"];
  for (const text of samples) {
    for (const [from, to] of typstSkips(text)) {
      assert.ok(0 <= from && from <= to && to <= text.length, `${JSON.stringify(text)}: ${from}..${to}`);
    }
  }
}

// A long line of completed inline raw spans is scanned in linear time, and its last word is kept.
{
  assert.deepEqual(words("`x` ".repeat(20000) + "word"), ["word"]);
}

console.log("spellcheck-typst: markup ranges the spellchecker skips");
