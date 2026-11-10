import assert from "node:assert/strict";
import test from "node:test";
import { extractOutline } from "../../src/lib/outline.js";

const headings = (source) => extractOutline(source, "org").map(({ title, level }) => ({ title, level }));

test("org outline lists headlines with their levels and cleaned titles", () => {
  const source = [
    "#+TITLE: Notes",
    "* Intro",
    "** TODO [#A] Methods :tag:",
    "* COMMENT hidden",
    "#+begin_src",
    // As in Org, a starred line is a headline even inside a block (Org escapes it as ,*).
    "* inside a block",
    "#+END_SRC",
    "*** Deep",
    "",
  ].join("\n");
  assert.deepEqual(headings(source), [
    { title: "Intro", level: 1 },
    { title: "Methods", level: 2 },
    { title: "inside a block", level: 1 },
    { title: "Deep", level: 3 },
  ]);
});

test("org outline reports offsets and lines of headlines", () => {
  const source = "#+TITLE: x\r\n* One\r\n** Two\r\n";
  assert.deepEqual(extractOutline(source, "org"), [
    { title: "One", level: 1, from: source.indexOf("* One"), line: 2 },
    { title: "Two", level: 2, from: source.indexOf("** Two"), line: 3 },
  ]);
});

test("org outline skips drawer contents and keeps headlines after them", () => {
  const source = "* Top\n:PROPERTIES:\n:CUSTOM_ID: top\n:END:\n* Next\n:LOGBOOK:\n- note\n:END:\n* Last\n";
  assert.deepEqual(headings(source), [
    { title: "Top", level: 1 },
    { title: "Next", level: 1 },
    { title: "Last", level: 1 },
  ]);
});

test("org outline leaves a plain heading-like line alone", () => {
  assert.deepEqual(headings("*bold* text\n*\n** Real"), [{ title: "Real", level: 2 }]);
  assert.deepEqual(extractOutline("", "org"), []);
});
