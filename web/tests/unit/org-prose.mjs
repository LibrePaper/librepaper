// Org prose: the spellcheck skips, the word count and the declared language
// for `.org` sources.
import assert from "node:assert/strict";
import { skipRanges } from "../../src/lib/spellcheck/index.js";
import { countWords } from "../../src/lib/word-count.js";
import { declaredLanguage, isEnglishDocument } from "../../src/lib/harper/language.js";
import { loadTypstSyntaxForTests } from "../helpers/typst-syntax.mjs";

await loadTypstSyntaxForTests();

const sample = [
  "#+TITLE: A Study",
  "#+AUTHOR: Zzyzx Qwfp",
  "#+LANGUAGE: fr",
  "",
  "* TODO [#A] Methods :draft:",
  ":PROPERTIES:",
  ":CUSTOM_ID: meth",
  ":END:",
  "",
  "We used *bold* text, ~kode~, $xqz$, [[https://exmpl.org][a link]], [cite:@doe2020] and [fn::a note].",
  "",
  "#+BEGIN_SRC R",
  "zzfunc()",
  "#+END_SRC",
  "",
  "- first item",
  "- second item",
  "",
  "| alpha | beta |",
  "| gamma | delta |",
  "",
].join("\n");

// The text with every skipped range blanked, so only prose remains.
function proseOf(text) {
  const chars = text.split("");
  for (const [from, to] of skipRanges(text, "org")) {
    for (let i = from; i < to; i++) if (chars[i] !== "\n") chars[i] = " ";
  }
  return chars.join("");
}

// Prose that must survive the skips.
{
  const prose = proseOf(sample);
  for (const word of ["A Study", "Methods", "bold", "a link", "a note", "alpha", "beta", "gamma", "delta", "first", "second"]) {
    assert.ok(prose.includes(word), word);
  }
}

// Markup that must not reach the spellchecker.
{
  const prose = proseOf(sample);
  for (const word of ["Zzyzx", "TODO", "draft", "meth", "kode", "xqz", "exmpl", "doe2020", "zzfunc", "BEGIN_SRC"]) {
    assert.ok(!prose.includes(word), word);
  }
}

// Every range is inside the text, and the ranges are sorted and disjoint.
{
  const ranges = skipRanges(sample, "org");
  assert.ok(ranges.length > 0);
  for (const [from, to] of ranges) {
    assert.ok(Number.isInteger(from) && Number.isInteger(to));
    assert.ok(0 <= from && from < to && to <= sample.length);
  }
  for (let i = 1; i < ranges.length; i++) {
    assert.ok(ranges[i - 1][1] < ranges[i][0]);
  }
}

// Word count reads only the prose.
assert.equal(countWords("* Heading\n\nOne two three.\n\n#+BEGIN_SRC R\nx <- 1\n#+END_SRC\n", "org"), 4);

// The declared language is the #+LANGUAGE value, and French is not English.
assert.equal(declaredLanguage(sample, "org"), "fr");
assert.equal(isEnglishDocument(sample, "org"), false);
assert.equal(declaredLanguage("#+language: en\n* Hi", "org"), "en");
assert.equal(declaredLanguage("* Just a headline\n", "org"), null);

// An unterminated block runs to the end of the text without throwing.
{
  const text = "Before words.\n#+BEGIN_SRC R\nx <- 1\nmore code";
  assert.doesNotThrow(() => skipRanges(text, "org"));
  const prose = proseOf(text);
  assert.ok(prose.includes("Before words."));
  assert.ok(!prose.includes("more code"));
}
