import assert from "node:assert/strict";
import test from "node:test";
import { parseOrg } from "../../src/lib/org-language/parser.js";

/** The nodes of a parse as [name, text] pairs, in document order. */
function nodes(text) {
  const out = [];
  parseOrg(text).iterate({
    enter(node) {
      out.push([node.name, text.slice(node.from, node.to)]);
    },
  });
  return out;
}

const textOf = (text, name) => nodes(text).filter(([n]) => n === name).map(([, t]) => t);

test("headlines mark TODO, priority, tags and title", () => {
  const src = "** TODO [#A] Title :tag:";
  assert.deepEqual(textOf(src, "Stars"), ["**"]);
  assert.deepEqual(textOf(src, "TodoKeyword"), ["TODO"]);
  assert.deepEqual(textOf(src, "Priority"), ["[#A]"]);
  assert.deepEqual(textOf(src, "Title"), ["Title"]);
  assert.deepEqual(textOf(src, "Tag"), ["tag"]);
});

test("sections nest by headline level", () => {
  const src = "* A\ntext\n** B\n* C\n";
  assert.deepEqual(textOf(src, "Section"), ["* A\ntext\n** B", "** B", "* C"]);
});

test("keyword lines split the key from the value", () => {
  const src = "#+TITLE: Paper";
  assert.deepEqual(textOf(src, "KeywordKey"), ["TITLE"]);
  assert.deepEqual(textOf(src, "KeywordValue"), ["Paper"]);
});

test("source blocks have a verbatim body", () => {
  const src = "#+begin_src R\nx <- 1\n#+end_src";
  assert.deepEqual(textOf(src, "BlockName"), ["src"]);
  assert.deepEqual(textOf(src, "BlockLanguage"), ["R"]);
  assert.deepEqual(textOf(src, "VerbatimBody"), ["x <- 1"]);
  assert.deepEqual(textOf(src, "BlockEnd"), ["#+end_src"]);
});

test("bold, italic and code markup", () => {
  const src = "*bold* and /it/ and ~code~";
  assert.deepEqual(textOf(src, "Bold"), ["*bold*"]);
  assert.deepEqual(textOf(src, "Italic"), ["/it/"]);
  assert.deepEqual(textOf(src, "Code"), ["~code~"]);
});

test("emphasis inside a word does not open", () => {
  assert.deepEqual(textOf("a*b*c", "Bold"), []);
});

test("bracket links hold a target and a description", () => {
  const src = "[[https://x.org][site]]";
  assert.deepEqual(textOf(src, "LinkTarget"), ["https://x.org"]);
  assert.deepEqual(textOf(src, "LinkDescription"), ["site"]);
});

test("inline math", () => {
  assert.deepEqual(textOf("$x^2$,", "InlineMath"), ["$x^2$"]);
});

test("citations list their keys", () => {
  assert.deepEqual(textOf("[cite/t:@a1;@b-2]", "CitationKey"), ["a1", "b-2"]);
});

test("a long line of markup characters terminates and every node stays in range", () => {
  const line = "*/".repeat(2500);
  const tree = parseOrg(line);
  assert.equal(tree.length, line.length);
});
