import assert from "node:assert/strict";
import { test } from "node:test";
import { parser as latexParser } from "../../src/lib/latex-language/parser.js";
import { pandocMarkdownParser } from "../../src/lib/pandoc-markdown.js";
import { frontMatterRange, latexTree, markdownTree } from "../../src/lib/source-trees.js";

const LATEX = "\\section{One}\nSome $x$ text.\n\\begin{itemize}\n\\item a\n\\end{itemize}\n\\label{end}\n";
const MARKDOWN = "---\ntitle: T\n---\n\n# Head\n\nSome *text* with $x^2$.\n\n- a\n- b\n\n::: {.note}\nbody\n:::\n";

// Start, middle, end, deletion.
const edits = (text) => [
  "NEW\n" + text,
  text.slice(0, text.length >> 1) + "\\inserted{x}\n" + text.slice(text.length >> 1),
  text + "tail line\n",
  text.slice(0, 10) + text.slice(30),
];

test("memo returns the same object for identical text, whatever the key", () => {
  assert.equal(latexTree(LATEX, "a.tex"), latexTree(LATEX, "b.tex"));
  assert.equal(latexTree(LATEX), latexTree(LATEX, "a.tex"));
  assert.equal(markdownTree(MARKDOWN, "a.md"), markdownTree(MARKDOWN));
});

test("incremental LaTeX parse matches a from-scratch parse", () => {
  latexTree(LATEX, "inc.tex");
  for (const edited of edits(LATEX)) {
    assert.equal(latexTree(edited, "inc.tex").toString(), latexParser.parse(edited).toString());
    latexTree(LATEX, "inc.tex");
  }
});

test("incremental Markdown parse matches a from-scratch parse", () => {
  markdownTree(MARKDOWN, "inc.md");
  for (const edited of edits(MARKDOWN)) {
    const { tree, body } = markdownTree(edited, "inc.md");
    assert.equal(tree.toString(), pandocMarkdownParser.parse(body).toString());
    markdownTree(MARKDOWN, "inc.md");
  }
});

test("front matter is blanked, keeping length and offsets", () => {
  const { body, frontMatter } = markdownTree(MARKDOWN);
  assert.deepEqual(frontMatter, [0, MARKDOWN.indexOf("---\n\n") + 3]);
  assert.equal(body.length, MARKDOWN.length);
  assert.equal(body.slice(0, frontMatter[1]), MARKDOWN.slice(0, frontMatter[1]).replace(/[^\n]/g, " "));
  assert.equal(body.slice(frontMatter[1]), MARKDOWN.slice(frontMatter[1]));
});

test("CRLF becomes space plus newline, lone CR becomes newline, length kept", () => {
  const text = "a\r\nb\rc\n";
  const { body } = markdownTree(text);
  assert.equal(body, "a \nb\nc\n");
  assert.equal(body.length, text.length);
  const yaml = "---\r\nk: v\r\n---\r\ntext\r\n";
  const out = markdownTree(yaml);
  assert.equal(out.body.length, yaml.length);
  assert.equal(out.body, "    \n     \n    \ntext \n");
});

test("frontMatterRange", () => {
  assert.equal(frontMatterRange("# Title\n\ntext"), null);
  assert.equal(frontMatterRange(""), null);
  assert.deepEqual(frontMatterRange("---\na: 1\n---\nx"), [0, 12]);
  assert.deepEqual(frontMatterRange("---\na: 1\n...\nx"), [0, 12]);
  assert.deepEqual(frontMatterRange("---\na: 1\n---"), [0, 12]);
  assert.equal(frontMatterRange("---\na: 1\nno close\n"), null);
  assert.equal(frontMatterRange("\n---\na: 1\n---\n"), null);
});
