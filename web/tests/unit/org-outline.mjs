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

const fields = (heading) => {
  const { title, todo, done, priority, tags } = heading;
  return { title, todo, done, priority, tags };
};

test("org outline reports the TODO state, priority and tags of a headline", () => {
  const source = "* TODO [#A] Write :work:home:\n* DONE Ship\n* Plain :work:\n* Bare\n";
  assert.deepEqual(extractOutline(source, "org").map(fields), [
    { title: "Write", todo: "TODO", done: false, priority: "A", tags: ["work", "home"] },
    { title: "Ship", todo: "DONE", done: true, priority: undefined, tags: undefined },
    { title: "Plain", todo: undefined, done: undefined, priority: undefined, tags: ["work"] },
    { title: "Bare", todo: undefined, done: undefined, priority: undefined, tags: undefined },
  ]);
});

test("org outline adds only the fields a headline has", () => {
  const [bare] = extractOutline("* Bare\n", "org");
  assert.deepEqual(Object.keys(bare).sort(), ["from", "level", "line", "title"]);
});

test("org outline reads done states declared by TODO keyword lines", () => {
  const source = [
    "#+TODO: NEXT(n) | FINISHED(f) CANCELED",
    "#+SEQ_TODO: A B C",
    "* NEXT one",
    "* FINISHED two",
    "* CANCELED three",
    "* TODO four",
    "* DONE five",
    "* A six",
    "* C seven",
    "",
  ].join("\n");
  assert.deepEqual(extractOutline(source, "org").map(({ todo, done }) => ({ todo, done })), [
    { todo: "NEXT", done: false },
    { todo: "FINISHED", done: true },
    { todo: "CANCELED", done: true },
    { todo: "TODO", done: false },
    { todo: "DONE", done: true },
    { todo: "A", done: false },
    { todo: "C", done: true },
  ]);
});

test("other formats do not report Org fields", () => {
  const sources = {
    markdown: "# TODO [#A] Plan :work:\n",
    typst: "= TODO Plan :work:\n",
    html: "<h1>TODO Plan</h1>\n",
    latex: "\\section{TODO Plan}\n",
  };
  for (const [format, source] of Object.entries(sources)) {
    const [heading] = extractOutline(source, format);
    assert.ok(heading, format);
    assert.deepEqual(Object.keys(heading).sort(), ["from", "level", "line", "title"], format);
  }
});
