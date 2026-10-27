// Word counts over the blanked source, the selection and section helpers, and
// the document walk that follows includes.
import assert from "node:assert/strict";
import {
  countWords,
  countWordsIn,
  documentFiles,
  documentWords,
  sectionWords,
} from "../../src/lib/word-count.js";
import { includesOf } from "../../src/lib/opening.js";

// LaTeX: commands and comments are skipped, prose and caption text count.
{
  assert.equal(countWords("\\section{Introduction} Some text $x+y$ here. % comment", "latex"), 4);
  const body = "Before text.\n\\begin{equation}\nx = y foo\n\\end{equation}\nAfter.";
  assert.equal(countWords(body, "latex"), 3);
  assert.equal(countWords("\\caption{Two words}", "latex"), 2);
  // Include names are markup in both braced and unbraced forms.
  assert.equal(countWords("\\input{b} x", "latex"), 1);
  assert.equal(countWords("\\input b.tex x", "latex"), 1);
  assert.equal(countWords("\\subfile{b} x", "latex"), 1);
}

// Typst: headings count, math is skipped.
{
  assert.equal(countWords("= Title\n\nSome $x^2 + y$ text.", "typst"), 3);
}

// Markdown: fenced code is skipped, apostrophes and hyphens join words.
{
  const fenced = "Intro words here.\n\n```\nprint(1) foo\n```\n\nAfter text.\n";
  assert.equal(countWords(fenced, "markdown"), 5);
  assert.equal(countWords("Don't stop: it's well-known.\n", "markdown"), 4);
}

// Ordered-list markers are not words, but numbers inside the prose still count.
{
  assert.equal(countWords("1. Alpha\n2. Beta", "markdown"), 2);
  assert.equal(countWords("In 2020 we ran 3 trials.", "markdown"), 6);
  assert.equal(countWords("1) Alpha\n2) Beta", "quarto"), 2);
}

// HTML: tags are skipped and the text between them counts.
{
  assert.equal(countWords("<p>Hello <b>big</b> world</p>", "html"), 3);
}

// CJK: each Han character is its own word, Latin words still count.
{
  assert.equal(countWords("\u65e5\u672c\u8a9e text", "latex"), 4);
}

// countWordsIn counts the prose inside a selection, and a cut word counts once.
{
  const text = "Alpha beta. Gamma delta.";
  assert.equal(countWordsIn(text, "latex", text.indexOf("beta"), text.length), 3);
  assert.equal(countWordsIn(text, "latex", text.indexOf("eta"), text.length), 3);
  assert.equal(countWordsIn(text, "latex", 0, text.indexOf("beta")), 1);
}

// Section counts sum to the count from the first heading on.
{
  const text = "\\section{One} alpha beta\n\\section{Two} gamma\n\\section{Three} delta epsilon zeta";
  const headings = ["\\section{One}", "\\section{Two}", "\\section{Three}"].map((marker) => ({
    from: text.indexOf(marker),
  }));
  const counts = sectionWords(text, "latex", headings);
  assert.deepEqual(counts, [3, 2, 4]);
  assert.equal(
    counts.reduce((sum, n) => sum + n, 0),
    countWords(text.slice(headings[0].from), "latex"),
  );
}

// A document follows \input and #include, each file once and in source order.
{
  const texts = {
    "thesis.tex":
      "\\documentclass{article}\n\\begin{document}\nIntro words.\n\\input{chap}\n\\end{document}",
    "chap.tex": "Chapter text.",
    "book.typ": '#include "a.typ"\n= Book\n',
    "a.typ": "Alpha.",
  };
  assert.deepEqual(documentFiles("thesis.tex", texts), ["thesis.tex", "chap.tex"]);
  assert.deepEqual(documentFiles("book.typ", texts), ["book.typ", "a.typ"]);
  assert.deepEqual(documentFiles("missing.tex", texts), []);
  const thesis = documentWords("thesis.tex", texts);
  assert.equal(thesis.words, 4);
  assert.deepEqual(thesis.files.map((file) => file.path), ["thesis.tex", "chap.tex"]);
  assert.equal(thesis.files[1].words, 2, "the chapter counts its own words");
  assert.equal(thesis.words, thesis.files[0].words + thesis.files[1].words);
}

// A cycle does not loop: each file is visited once.
{
  const texts = { "a.tex": "\\input{b}", "b.tex": "\\input{a}" };
  assert.deepEqual(documentFiles("a.tex", texts), ["a.tex", "b.tex"]);
}

// includesOf keeps the source order, drops duplicates, self and missing files.
{
  const texts = {
    "main.tex": "\\input{b}\n\\input{missing}\n\\input{a}\n\\input{b}\n\\input{main}",
    "a.tex": "",
    "b.tex": "",
  };
  assert.deepEqual(includesOf("main.tex", texts), ["b.tex", "a.tex"]);
  assert.deepEqual(includesOf("a.tex", texts), []);
  assert.deepEqual(includesOf("notes.txt", texts), []);
}

// Includes of different commands come back in source order.
{
  const texts = { "main.tex": "\\include{a}\n\\input{b}", "a.tex": "", "b.tex": "" };
  assert.deepEqual(includesOf("main.tex", texts), ["a.tex", "b.tex"]);
}
