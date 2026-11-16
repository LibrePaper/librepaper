import assert from "node:assert/strict";
import { test } from "node:test";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { ensureSyntaxTree, foldable } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { TreeFragment } from "@lezer/common";
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

// Every node with its positions, one per line, so whole trees compare as strings.
function shape(tree) {
  const found = [];
  tree.iterate({
    enter(node) {
      if (node.name !== "Document") found.push(`${node.name} ${node.from} ${node.to}`);
    },
  });
  return found.join("\n");
}

// Reparsing a document with its own tree as fragments must give the fresh tree.
function assertIncremental(text) {
  const fresh = shape(parser.parse(text));
  const incremental = shape(parser.parse(text, TreeFragment.addTree(parser.parse(text))));
  assert.equal(incremental, fresh);
}

test("inline math follows Pandoc's dollar rules", () => {
  assert.deepEqual(texts("a $x^2$ b", "InlineMath"), ["$x^2$"]);
  assert.deepEqual(names("$x^2$", ["InlineMathMark"]), ["InlineMathMark", "InlineMathMark"]);
  assert.deepEqual(texts("costs $5 and $6", "InlineMath"), []);
  assert.deepEqual(texts("\\$5 and \\$6", "InlineMath"), []);
  assert.deepEqual(texts("$ x$", "InlineMath"), []);
  assert.deepEqual(texts("$x $", "InlineMath"), []);
  assert.deepEqual(texts("$x$5", "InlineMath"), []);
  assert.deepEqual(texts("$a\\$b$", "InlineMath"), ["$a\\$b$"]);
  assert.deepEqual(texts("$5 and $10 and x$", "InlineMath"), ["$10 and x$"]);
  assert.deepEqual(texts("$a $b$", "InlineMath"), ["$b$"]);
  assert.deepEqual(texts("$x$5 y$", "InlineMath"), ["$5 y$"]);
  assert.deepEqual(texts("$a \\$ b$", "InlineMath"), ["$a \\$ b$"]);
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

test("fenced divs nest", () => {
  const doc = "::: outer\n\n::::: {.inner}\nx\n:::::\n\ny\n:::";
  assert.deepEqual(texts(doc, "FencedDiv"), [doc, "::::: {.inner}\nx\n:::::"]);
});

test("an opener without a closing fence is paragraph text", () => {
  const open = "::: note\nbody *em*\n\nmore";
  assert.deepEqual(texts(open, "FencedDiv"), []);
  assert.deepEqual(texts(open, "Emphasis"), ["*em*"]);
  assert.deepEqual(names("::: note\nbody", ["FencedDivMark"]), []);
});

test("a closing fence ends an indented code block and the div, in fresh and incremental parses", () => {
  const doc = "::: note\n    code\n:::\n\nafter";
  assert.deepEqual(texts(doc, "FencedDiv"), ["::: note\n    code\n:::"]);
  assert.deepEqual(texts(doc, "FencedDivMark"), [":::", ":::"]);
  assert.deepEqual(texts(doc, "CodeBlock"), ["code"]);
  assert.deepEqual(texts(doc, "Paragraph"), ["after"]);
  for (const text of [doc, "::: note\n    code\n:::\n", "::: note\nbody\n\nmore"]) assertIncremental(text);
});

test("a div closed inside a list item keeps its closing fence inside the div", () => {
  const doc = "- item\n\n  ::: note\n  text\n  :::\n  after\n";
  assert.deepEqual(texts(doc, "FencedDiv"), ["::: note\n  text\n  :::"]);
  assert.deepEqual(texts(doc, "FencedDivMark"), [":::", ":::"]);
  assertIncremental(doc);
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

test("an indented :::  is code, not a closing fence", () => {
  const doc = "::: note\nText\n\n    :::\n    code\n\nMore\n:::\n";
  assert.deepEqual(texts(doc, "FencedDiv"), [doc.trimEnd()]);
  const state = EditorState.create({
    doc,
    extensions: markdown({ base: markdownLanguage, extensions: pandocExtensions }),
  });
  ensureSyntaxTree(state, state.doc.length, 1000);
  assert.deepEqual(foldable(state, 0, 8), { from: 8, to: doc.lastIndexOf(":::") });
  const tabbed = "::: note\nText\n\n\t:::\n\nMore\n:::";
  assert.deepEqual(texts(tabbed, "FencedDiv"), [tabbed]);
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
  // The link stays a link; Pandoc reads a key in link text as a citation too.
  assert.deepEqual(names("[@a](http://x)", ["Link", "Citation"]), ["Link", "Citation"]);
  assert.deepEqual(names("`@doe`", ["Citation"]), []);
});

test("a bracketed citation keeps inline parsing in its prefix and suffix", () => {
  const doc = "[see *discussion* @doe99, p. 3; $x^2$ @roe]";
  assert.deepEqual(texts(doc, "Citation"), [doc]);
  assert.deepEqual(texts(doc, "Emphasis"), ["*discussion*"]);
  assert.deepEqual(texts(doc, "InlineMath"), ["$x^2$"]);
  assert.deepEqual(texts(doc, "CitationKey"), ["@doe99", "@roe"]);
  assert.deepEqual(texts(doc, "CitationMark"), ["[", "]"]);
  assert.deepEqual(texts("[*@a*]", "CitationKey"), ["@a"]);
  assert.deepEqual(texts("text [@a] and @b", "Citation"), ["[@a]", "@b"]);
});

test("brackets without a key stay links, footnotes and spans", () => {
  assert.deepEqual(names("[text](x)", ["Link", "Citation"]), ["Link"]);
  assert.deepEqual(names("[^1] note", ["Citation"]), []);
  assert.deepEqual(names("[text]{.class}", ["Citation", "AttributeClass"]), ["AttributeClass"]);
  assert.deepEqual(names("[@a][ref]", ["Link", "CitationMark"]), ["Link"]);
  assert.deepEqual(names("[@a](http://x)", ["Link", "CitationMark"]), ["Link"]);
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

test("a div inside a list item closes at the item's indentation", () => {
  const doc = "10. ::: note\n    Inside.\n    :::\n\n    After the div.\n";
  const [div] = texts(doc, "FencedDiv");
  assert.equal(div, "::: note\n    Inside.\n    :::");
});

test("a quoted ::: inside a div does not close it", () => {
  const doc = "::: note\nText\n\n> :::\n\nMore\n:::\n";
  assert.deepEqual(texts(doc, "FencedDiv"), [doc.trimEnd()]);
  const quoted = "> ::: note\n> Inside.\n> :::\n\nAfter.\n";
  assert.deepEqual(texts(quoted, "FencedDiv"), ["::: note\n> Inside.\n> :::"]);
});
