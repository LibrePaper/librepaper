import assert from "node:assert/strict";
import test from "node:test";
import { highlightTree, tagHighlighter, tags } from "@lezer/highlight";
import { loadOrgSyntaxForTests } from "../helpers/org-syntax.mjs";

const api = await loadOrgSyntaxForTests();

// The highlight classes the node props in props.js resolve to, named after their tags.
const highlighter = tagHighlighter(
  ["heading", "keyword", "atom", "tagName", "meta", "string", "monospace", "comment", "strong", "emphasis", "link"]
    .map((name) => ({ tag: tags[name], class: name })),
);

/** For each UTF-16 offset of the text, the set of highlight classes covering it. */
function classesAt(text) {
  const per = new Array(text.length);
  highlightTree(api.parse(text), highlighter, (from, to, cls) => {
    for (let i = from; i < to; i++) (per[i] ??= new Set()).add(cls);
  });
  return per;
}

/** Asserts that every character of the first occurrence of `piece` carries class `cls`. */
function assertClass(text, piece, cls) {
  const at = text.indexOf(piece);
  assert.ok(at >= 0, `${JSON.stringify(piece)} is not in ${JSON.stringify(text)}`);
  const per = classesAt(text);
  for (let i = at; i < at + piece.length; i++) {
    assert.ok(per[i]?.has(cls), `${JSON.stringify(piece)} should be ${cls}, got ${[...(per[i] ?? [])].join(",") || "nothing"}`);
  }
}

/** The nodes of a parse as [name, text] pairs, in document order. */
function nodes(text) {
  const out = [];
  api.parse(text).iterate({
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
  const tree = api.parse(line);
  assert.equal(tree.length, line.length);
});

test("highlighting: headline parts", () => {
  assertClass("* Ship it", "Ship it", "heading");
  assertClass("* TODO Ship", "TODO", "keyword");
  assertClass("* [#A] Ship", "[#A]", "atom");
  assertClass("* Ship :work:", ":work:", "tagName");
});

test("highlighting: keyword lines, blocks and verbatim bodies", () => {
  assertClass("#+TITLE: Paper", "TITLE", "meta");
  assertClass("#+begin_src R\nx <- 1\n#+end_src", "x <- 1", "monospace");
});

test("highlighting: inline markup", () => {
  assertClass("*bold*", "*bold*", "strong");
  assertClass("/it/", "/it/", "emphasis");
  assertClass("~code~", "~code~", "monospace");
  assertClass("[[https://x.org][site]]", "https://x.org", "link");
  assertClass("$x^2$,", "$x^2$", "string");
});

test("highlighting: comment lines", () => {
  assertClass("# a comment\n", "a comment", "comment");
});
