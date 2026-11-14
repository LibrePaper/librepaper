import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { loadOrgSyntax } from "../../src/lib/org-language/module.js";
import { loadTypstSyntax } from "../../src/lib/typst-language/module.js";
import { latexFolds, markdownFolds, orgFolds, typstFolds } from "../../src/lib/visual/fold-finders.js";

// The folds as plain records, so that a test reads what the editor will hide.
const plain = (folds, text) =>
  folds.map(({ from, to, kind, detail, target }) => ({
    kind,
    text: text.slice(from, to).split("\n")[0],
    detail,
    ...(target === undefined ? {} : { target }),
  }));

// Folds are sorted, never overlap, and each covers whole lines.
function assertWellFormed(folds, text) {
  for (let i = 0; i < folds.length; i++) {
    const fold = folds[i];
    assert.ok(fold.from === 0 || text[fold.from - 1] === "\n", `starts a line: ${fold.from}`);
    assert.ok(fold.to === text.length || text[fold.to] === "\n", `ends a line: ${fold.to}`);
    assert.ok(fold.detail === undefined || fold.detail.length <= 60, `short detail: ${fold.detail}`);
    if (i > 0) assert.ok(folds[i - 1].to < fold.from, "sorted and not overlapping");
  }
}

// ---- LaTeX ----------------------------------------------------------------

{
  const text = [
    "\\documentclass{article}",
    "\\usepackage{amsmath,graphicx}",
    "\\title{T}",
    "\\begin{document}",
    "\\input{chapters/one}  % the first",
    "\\include{two}",
    "% a note",
    "% another",
    "Text.",
    "\\begin{verbatim}",
    "% not a comment",
    "\\end{verbatim}",
    "",
  ].join("\n");
  const folds = latexFolds(text);
  assertWellFormed(folds, text);
  // The preamble runs to the line before \begin{document}, and names its class.
  assert.equal(folds[0].kind, "preamble");
  assert.equal(folds[0].from, 0);
  assert.equal(text.slice(0, folds[0].to).split("\n").at(-1), "\\title{T}");
  assert.match(folds[0].detail, /article/);
  assert.deepEqual(plain(folds.slice(1), text), [
    { kind: "include", text: "\\input{chapters/one}  % the first", detail: "chapters/one", target: "chapters/one" },
    { kind: "include", text: "\\include{two}", detail: "two", target: "two" },
    { kind: "comment", text: "% a note", detail: "a note" },
  ]);
  // The verbatim body is text, so its % line is not a comment fold.
  assert.equal(folds.length, 4);
  assert.equal(latexFolds(text), latexFolds(text));
}

// ---- Markdown and Quarto --------------------------------------------------

{
  const text = [
    "---",
    'title: "Paper Title"',
    "---",
    "",
    "# Heading",
    "",
    "Text[^1] and [link][ref].",
    "",
    '{{< include "chapters/intro.qmd" >}}',
    "",
    "<!-- editor note: check this -->",
    "",
    "[ref]: https://example.org",
    "",
    "[^1]: A note.",
    "",
  ].join("\n");
  const folds = markdownFolds(text);
  assertWellFormed(folds, text);
  const named = plain(folds, text);
  assert.deepEqual(named[0], { kind: "preamble", text: "---", detail: "Paper Title" });
  assert.ok(named.some((fold) => fold.kind === "definitions" && fold.detail === "[ref]"));
  assert.ok(named.some((fold) => fold.kind === "definitions" && fold.detail === "[^1]"));
  assert.ok(named.some((fold) => fold.kind === "comment" && fold.detail === "editor note: check this"));
  assert.ok(
    named.some((fold) => fold.kind === "include" && fold.detail === "chapters/intro.qmd" && fold.target === "chapters/intro.qmd"),
  );
  assert.equal(named.length, 5, JSON.stringify(named));
  assert.equal(folds[0].others, "");
}

// The front matter keys that a title block does not show are named beside its pill.
{
  const text = [
    "---",
    "title: T",
    "author:",
    "  - name: A",
    "bibliography: refs.bib",
    "format:",
    "  html: default",
    "date: 2026-01-01",
    "bibliography: again.bib",
    "---",
    "",
    "Body.",
    "",
  ].join("\n");
  const [front] = markdownFolds(text);
  assert.equal(front.kind, "preamble");
  assert.equal(front.detail, "T · bibliography · format");
  assert.equal(front.others, "bibliography · format");
}

// A front matter without a title names the keys it hides.
{
  const text = ["---", "bibliography: references.bib", "bibliography-style: apa", "---", "", "# Heading", ""].join("\n");
  const [front] = markdownFolds(text);
  assert.equal(front.kind, "preamble");
  assert.equal(front.detail, "bibliography · bibliography-style");
}

// ---- Org ------------------------------------------------------------------

// Before the syntax module loads, the finder has nothing to say. That is not
// cached: once the module is loaded, the same text gets its folds.
const lateHeader = "#+OPTIONS: toc:nil\n#+STARTUP: overview\n\nBody.\n";
assert.deepEqual(orgFolds(lateHeader), []);

// The syntax module must be loaded before the tree can be read.
await loadOrgSyntax(readFileSync(new URL("../../wasm/org.wasm", import.meta.url)));

{
  const folds = orgFolds(lateHeader);
  assert.equal(folds.length, 1);
  assert.equal(folds[0].kind, "preamble");
  assert.equal(folds[0].detail, "#+OPTIONS · #+STARTUP");
}

{
  const text = [
    "#+TITLE: My Doc",
    '#+INCLUDE: "chapters/intro.org::*Start"',
    "#+AUTHOR: A",
    "",
    "* Heading",
    ":PROPERTIES:",
    ":ID: x",
    ":END:",
    "",
    "#+NAME: table-one",
    "| a | b |",
    "",
    "# a comment",
    "#+begin_comment",
    "second",
    "#+end_comment",
    "",
    "[fn:1] Note.",
    "",
  ].join("\n");
  const folds = orgFolds(text);
  assertWellFormed(folds, text);
  assert.equal(folds[0].others, "");
  const named = plain(folds, text);
  assert.deepEqual(named, [
    { kind: "preamble", text: "#+TITLE: My Doc", detail: "My Doc" },
    { kind: "include", text: '#+INCLUDE: "chapters/intro.org::*Start"', detail: "chapters/intro.org", target: "chapters/intro.org" },
    { kind: "preamble", text: "#+AUTHOR: A", detail: "#+AUTHOR" },
    { kind: "definitions", text: ":PROPERTIES:", detail: "PROPERTIES" },
    // A #+NAME: keyword is attached to the table below it, so it no longer folds on its own.
    { kind: "comment", text: "# a comment", detail: "a comment" },
    { kind: "definitions", text: "[fn:1] Note.", detail: "[fn:1]" },
  ]);
}

// A header with a title is one preamble fold that names its title and other keywords.
{
  const text = ["#+TITLE: Notes", "# draft", "#+AUTHOR: A", "#+OPTIONS: toc:nil", "", "Body text.", ""].join("\n");
  const folds = orgFolds(text);
  assertWellFormed(folds, text);
  assert.equal(folds.length, 1);
  assert.equal(folds[0].kind, "preamble");
  assert.equal(folds[0].detail, "Notes · #+OPTIONS");
  assert.equal(folds[0].others, "#+OPTIONS");
  assert.equal(
    text.slice(folds[0].from, folds[0].to),
    ["#+TITLE: Notes", "# draft", "#+AUTHOR: A", "#+OPTIONS: toc:nil"].join("\n"),
  );
}

// The other keywords of a title block are named once each, with their optional parts cut off.
{
  const text = ["#+TITLE: T", "#+bibliography: refs.bib", "#+OPTIONS: toc:nil", "#+options: num:nil", "", "Body.", ""].join("\n");
  const folds = orgFolds(text);
  assertWellFormed(folds, text);
  assert.equal(folds.length, 1);
  assert.equal(folds[0].detail, "T · #+BIBLIOGRAPHY · #+OPTIONS");
  assert.equal(folds[0].others, "#+BIBLIOGRAPHY · #+OPTIONS");
}

// An Org header of several keywords, with a comment among them, and no title keywords is one fold.
{
  const text = ["#+OPTIONS: toc:nil", "# draft", "#+STARTUP: overview", "#+LATEX_HEADER: \\usepackage{x}", "", "Body text.", ""].join("\n");
  const folds = orgFolds(text);
  assertWellFormed(folds, text);
  assert.equal(folds.length, 1);
  assert.equal(folds[0].kind, "preamble");
  assert.equal(folds[0].detail, "#+OPTIONS · #+STARTUP · #+LATEX_HEADER");
  assert.equal(
    text.slice(folds[0].from, folds[0].to),
    ["#+OPTIONS: toc:nil", "# draft", "#+STARTUP: overview", "#+LATEX_HEADER: \\usepackage{x}"].join("\n"),
  );
}

// ---- Typst ----------------------------------------------------------------

const typstWasm = new URL("../../wasm/typst-syntax.wasm", import.meta.url);
if (!existsSync(typstWasm)) {
  console.log("typst-syntax.wasm is missing; skipping the Typst fold tests");
} else {
  const text = [
    '#import "@preview/note:1.0.0": note',
    "#set text(size: 11pt)",
    "#show heading: set block(above: 2em)",
    "// a leftover comment",
    "",
    "Some text.",
    "",
    "#let later = 1",
    "",
    '#include "chapters/two.typ"',
    "",
    "// trailing comment",
    "",
  ].join("\n");
  // Before the module loads the finder returns nothing, and that is not cached.
  assert.deepEqual(typstFolds(text), []);
  await loadTypstSyntax(new Uint8Array(readFileSync(typstWasm)));
  const folds = typstFolds(text);
  assertWellFormed(folds, text);
  const named = plain(folds, text);
  assert.equal(named[0].kind, "preamble");
  assert.equal(named[0].detail, "#import · #set · #show");
  // A comment after the last setup rule is not part of the preamble.
  assert.deepEqual(named.slice(1), [
    { kind: "comment", text: "// a leftover comment", detail: "a leftover comment" },
    { kind: "definitions", text: "#let later = 1", detail: "#let" },
    { kind: "include", text: '#include "chapters/two.typ"', detail: "chapters/two.typ", target: "chapters/two.typ" },
    { kind: "comment", text: "// trailing comment", detail: "trailing comment" },
  ]);
}
