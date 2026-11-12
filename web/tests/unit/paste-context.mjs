// Which pastes insert plain text: math and code in each source format, and
// every paste in an HTML or unknown file.
import assert from "node:assert/strict";
import { test } from "node:test";
import { pastesPlainAt } from "../../src/lib/paste-context.js";

/* ------------------------------------------------------------- the helpers */

// The document with a `|` marking the caret, then the caret's offset.
function at(marked, format) {
  const idx = marked.indexOf("|");
  const text = marked.slice(0, idx) + marked.slice(idx + 1);
  return pastesPlainAt(text, idx, format);
}

/* ------------------------------------------------------------------ LaTeX */

test("latex: prose is not plain, math and verbatim are", () => {
  assert.equal(at("Plain paragraph| text.", "latex"), false);
  assert.equal(at("Text $x| + y$ more", "latex"), true);
  assert.equal(at("\\begin{equation}\nx| \n\\end{equation}", "latex"), true);
  assert.equal(at("\\begin{verbatim}\nx| \n\\end{verbatim}", "latex"), true);
  assert.equal(at("\\begin{lstlisting}\nfoo|", "latex"), true);
  assert.equal(at("\\begin{verbatim}\na\n\\end{verbatim}\nText|", "latex"), false);
});

test("latex: an escaped percent is text, a real comment is not", () => {
  assert.equal(at("Use 50\\%| off", "latex"), false);
  assert.equal(at("Done % comment|", "latex"), true);
  assert.equal(at("Line one % note\nLine two|", "latex"), false);
});

/* --------------------------------------------------------------- Markdown */

test("markdown: fenced code is plain, the text after the fence is not", () => {
  assert.equal(at("```js\nconst x| = 1;\n```", "markdown"), true);
  assert.equal(at("~~~\ncode|\n~~~", "markdown"), true);
  assert.equal(at("```js\ncode\n```\nAfter|", "markdown"), false);
  assert.equal(at("Some text|", "markdown"), false);
});

test("markdown: inline code and inline math on the current line", () => {
  assert.equal(at("Use `code|` here", "markdown"), true);
  assert.equal(at("Cost \\$5| now", "markdown"), false);
  assert.equal(at("Cost $x| + y$", "markdown"), true);
  assert.equal(at("Cost $x$ and text|", "markdown"), false);
});

test("markdown: display math and front matter", () => {
  assert.equal(at("$$\nx|\n$$", "markdown"), true);
  assert.equal(at("$$\na\n$$\nText|", "markdown"), false);
  assert.equal(at("---\ntitle: x|\n---\n", "markdown"), true);
  assert.equal(at("---\ntitle: x\n---\nBody|", "markdown"), false);
});

test("quarto: uses the Markdown rules", () => {
  assert.equal(at("```{python}\nx| = 1\n```", "quarto"), true);
  assert.equal(at("Plain| text", "quarto"), false);
});

/* ------------------------------------------------------------------ Typst */

test("typst: math and raw text are plain, prose is not", () => {
  assert.equal(at("$x| + y$", "typst"), true);
  assert.equal(at("$x$ text|", "typst"), false);
  assert.equal(at("```\ncode|\n```", "typst"), true);
  assert.equal(at("Use `raw|` here", "typst"), true);
  assert.equal(at("Text| plain", "typst"), false);
});

test("typst: line comments and escaped dollars", () => {
  assert.equal(at("// note|", "typst"), true);
  assert.equal(at("Price \\$5| ok", "typst"), false);
  assert.equal(at("/* a| */ text", "typst"), true);
  assert.equal(at("/* a */ text|", "typst"), false);
});

/* -------------------------------------------------------------------- Org */

test("org: source, example and export blocks are plain", () => {
  assert.equal(at("#+begin_src python\nprint|\n#+end_src", "org"), true);
  assert.equal(at("#+BEGIN_EXAMPLE\nx|\n#+END_EXAMPLE", "org"), true);
  assert.equal(at("#+begin_export html\n<p>|\n#+end_export", "org"), true);
  assert.equal(at("#+begin_src python\nx\n#+end_src\nText|", "org"), false);
});

test("org: inline and display LaTeX fragments", () => {
  assert.equal(at("Inline $x| + y$", "org"), true);
  assert.equal(at("Inline \\(x|\\)", "org"), true);
  assert.equal(at("\\[\nx|\n\\]", "org"), true);
  assert.equal(at("\\begin{equation}\nx|\n\\end{equation}", "org"), true);
  assert.equal(at("Plain| text", "org"), false);
});

/* ------------------------------------------------------- HTML and unknown */

test("html and unknown formats always take plain text", () => {
  assert.equal(at("<p>x|</p>", "html"), true);
  assert.equal(at("Plain| text", "html"), true);
  assert.equal(at("Plain| text", ""), true);
  assert.equal(at("Plain| text", "rst"), true);
});
