import assert from "node:assert/strict";
import { test } from "node:test";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { ensureSyntaxTree, foldable } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { pandocExtensions } from "../../src/lib/pandoc-markdown.js";

const parser = markdown({ base: markdownLanguage, extensions: pandocExtensions }).language.parser;

// Every node in the tree as a [name, text] pair, in document order.
function nodes(text, only) {
  const found = [];
  parser.parse(text).iterate({
    enter(node) {
      if (node.name === "Document") return;
      if (!only || only.includes(node.name)) found.push([node.name, text.slice(node.from, node.to)]);
    },
  });
  return found;
}
const names = (text, only) => nodes(text, only).map(([name]) => name);
const texts = (text, name) => nodes(text, [name]).map(([, body]) => body);

test("inline math follows Pandoc's dollar rules", () => {
  assert.deepEqual(texts("a $x^2$ b", "InlineMath"), ["$x^2$"]);
  assert.deepEqual(names("$x^2$", ["InlineMathMark"]), ["InlineMathMark", "InlineMathMark"]);
  assert.deepEqual(texts("costs $5 and $6", "InlineMath"), []);
  assert.deepEqual(texts("\\$5 and \\$6", "InlineMath"), []);
  assert.deepEqual(texts("$ x$", "InlineMath"), []);
  assert.deepEqual(texts("$x $", "InlineMath"), []);
  assert.deepEqual(texts("$x$5", "InlineMath"), []);
  assert.deepEqual(texts("$a\\$b$", "InlineMath"), ["$a\\$b$"]);
  assert.deepEqual(texts("$a_1 * b_2$", "Emphasis"), []);
});

test("display math, inline and in blocks, is not parsed as Markdown", () => {
  assert.deepEqual(texts("see $$a_1 * b_2$$ now", "DisplayMath"), ["$$a_1 * b_2$$"]);
  assert.deepEqual(names("see $$a_1 * b_2$$ now", ["Emphasis"]), []);
  const block = "$$\n\\int a_1 * b_2\n- c\n$$\n\nafter";
  assert.deepEqual(texts(block, "DisplayMath"), ["$$\n\\int a_1 * b_2\n- c\n$$"]);
  assert.deepEqual(names(block, ["Emphasis", "BulletList"]), []);
  assert.deepEqual(names("$$\nx\n$$ {#eq-a}", ["Attributes", "AttributeId"]), ["Attributes", "AttributeId"]);
  // An unfinished $$ never swallows the document.
  assert.deepEqual(names("$$\nx\n\nplain *text*", ["DisplayMath", "Emphasis"]), ["Emphasis"]);
});

test("fenced divs hold Markdown blocks", () => {
  const doc = "::: {.callout-note}\ntext *em*\n:::\n\nafter";
  assert.deepEqual(texts(doc, "FencedDiv"), ["::: {.callout-note}\ntext *em*\n:::"]);
  assert.deepEqual(texts(doc, "FencedDivMark"), [":::", ":::"]);
  assert.deepEqual(texts(doc, "Attributes"), ["{.callout-note}"]);
  assert.deepEqual(texts(doc, "AttributeClass"), [".callout-note"]);
  assert.deepEqual(texts(doc, "Emphasis"), ["*em*"]);
  assert.deepEqual(texts("::: note\nx\n:::", "AttributeClass"), ["note"]);
  assert.deepEqual(texts(":::{#fig-x}\nx\n:::", "AttributeId"), ["#fig-x"]);
  assert.deepEqual(texts("a :::\nb", "FencedDiv"), []);
});

test("fenced divs nest and may be left open", () => {
  const doc = "::: outer\n\n::::: {.inner}\nx\n:::::\n\ny\n:::";
  assert.deepEqual(texts(doc, "FencedDiv"), [doc, "::::: {.inner}\nx\n:::::"]);
  const open = "::: note\nbody *em*\n\nmore";
  assert.deepEqual(texts(open, "FencedDiv"), [open]);
  assert.deepEqual(texts(open, "Emphasis"), ["*em*"]);
});

test("a fenced div folds from its opening line to its closing line", () => {
  const doc = "::: note\nx\ny\n:::\n";
  const state = EditorState.create({
    doc,
    extensions: markdown({ base: markdownLanguage, extensions: pandocExtensions }),
  });
  ensureSyntaxTree(state, state.doc.length, 1000);
  assert.deepEqual(foldable(state, 0, 8), { from: 8, to: doc.indexOf(":::\n", 8) });
});

test("citations", () => {
  const doc = "[@doe99, p. 3; @roe]";
  assert.deepEqual(texts(doc, "CitationKey"), ["@doe99", "@roe"]);
  assert.deepEqual(texts(doc, "Citation"), [doc]);
  assert.deepEqual(texts("[see @doe99, p. 3]", "CitationKey"), ["@doe99"]);
  assert.deepEqual(texts("@doe99 says", "CitationKey"), ["@doe99"]);
  assert.deepEqual(texts("as @doe99.", "CitationKey"), ["@doe99"]);
  assert.deepEqual(names("mail a@b.org", ["Citation"]), []);
  assert.deepEqual(names("see @fig-a", ["Citation", "CrossRef"]), ["CrossRef"]);
  assert.deepEqual(names("[link](http://x)", ["Link", "Citation"]), ["Link"]);
  assert.deepEqual(names("[@a](http://x)", ["Link", "Citation"]), ["Link"]);
  assert.deepEqual(names("`@doe`", ["Citation"]), []);
});

test("attribute lists", () => {
  assert.deepEqual(texts("# Heading {#sec-intro}", "Attributes"), ["{#sec-intro}"]);
  assert.deepEqual(texts("# Heading {#sec-intro}", "AttributeId"), ["#sec-intro"]);
  const image = "![Cap](a.png){#fig-a width=50% title=\"v w\"}";
  assert.deepEqual(texts(image, "Attributes"), ["{#fig-a width=50% title=\"v w\"}"]);
  assert.deepEqual(texts(image, "AttributeKey"), ["width", "title"]);
  assert.deepEqual(texts(image, "AttributeValue"), ["50%", "\"v w\""]);
  assert.deepEqual(names(image, ["Image"]), ["Image"]);
  assert.deepEqual(texts("[text]{.smallcaps}", "AttributeClass"), [".smallcaps"]);
  assert.deepEqual(names("a {b} c", ["Attributes"]), []);
});

test("GFM and code are unaffected", () => {
  assert.deepEqual(names("| a | b |\n|---|---|\n| 1 | 2 |", ["Table"]), ["Table"]);
  assert.deepEqual(names("~~gone~~", ["Strikethrough"]), ["Strikethrough"]);
  assert.deepEqual(names("- [ ] todo", ["TaskMarker"]), ["TaskMarker"]);
  assert.deepEqual(names("```\n$x$ and @key\n```", ["InlineMath", "Citation"]), []);
  assert.deepEqual(names("`$x$`", ["InlineMath"]), []);
});
