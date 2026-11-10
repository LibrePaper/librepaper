import assert from "node:assert/strict";
import test from "node:test";
import { StringStream } from "@codemirror/language";
import { orgStreamParser } from "../../src/lib/org-language.js";

// Tokenize one line with the stream parser, carrying `state` across lines.
function tokens(line, state = orgStreamParser.startState()) {
  const stream = new StringStream(line, 2, 2);
  const out = [];
  while (!stream.eol()) {
    stream.start = stream.pos;
    const style = orgStreamParser.token(stream, state);
    out.push([line.slice(stream.start, stream.pos), style]);
    // Every token must consume input, or this assertion fails instead of looping.
    assert.ok(out.length <= line.length, "each token must consume at least one character");
  }
  return out;
}

function styleOf(pairs, text) {
  const found = pairs.find(([t]) => t.trim() === text);
  assert.ok(found, `no token ${JSON.stringify(text)} in ${JSON.stringify(pairs)}`);
  return found[1];
}

test("headlines mark TODO, priority, tags and title", () => {
  const pairs = tokens("** TODO [#A] Title :tag:");
  assert.equal(styleOf(pairs, "TODO"), "keyword");
  assert.equal(styleOf(pairs, "[#A]"), "atom");
  assert.equal(styleOf(pairs, ":tag:"), "tag");
  assert.equal(styleOf(pairs, "Title"), "heading");
});

test("keyword lines split the key from the value", () => {
  assert.deepEqual(tokens("#+TITLE: Paper"), [
    ["#+TITLE:", "meta"],
    [" Paper", "string"],
  ]);
});

test("source blocks are monospace and close the block", () => {
  const state = orgStreamParser.startState();
  assert.deepEqual(tokens("#+begin_src R", state), [["#+begin_src R", "meta"]]);
  assert.deepEqual(tokens("x <- 1", state), [["x <- 1", "monospace"]]);
  assert.deepEqual(tokens("#+end_src", state), [["#+end_src", "meta"]]);
  assert.equal(state.block, null);
});

test("bold, italic and code markup", () => {
  const pairs = tokens("*bold* and /it/ and ~code~");
  assert.equal(styleOf(pairs, "*bold*"), "strong");
  assert.equal(styleOf(pairs, "/it/"), "emphasis");
  assert.equal(styleOf(pairs, "~code~"), "monospace");
});

test("emphasis inside a word does not open", () => {
  assert.equal(tokens("a*b*c").some(([, style]) => style === "strong"), false);
});

test("bracket links are one link token", () => {
  assert.deepEqual(tokens("[[https://x.org][site]]"), [["[[https://x.org][site]]", "link"]]);
});

test("inline math is a string token", () => {
  assert.equal(styleOf(tokens("$x^2$,"), "$x^2$"), "string");
});

test("a long line of markup characters terminates and keeps every character", () => {
  const line = "*/".repeat(2500);
  const pairs = tokens(line);
  assert.equal(pairs.map(([text]) => text).join(""), line);
});
