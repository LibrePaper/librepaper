import assert from "node:assert/strict";
import test from "node:test";
import { byteToUtf16, folder, symbolItems, headingsOf, typstOutline, stripLabel, SYMBOL_KINDS } from "../../src/lib/typst-symbols.js";
import { createLiveIndex } from "../../src/lib/document-index/live.js";

const bytes = (text) => new TextEncoder().encode(text).length;

test("byte offsets map to UTF-16 indexes", () => {
  const ascii = byteToUtf16("abc");
  assert.deepEqual([0, 1, 3, 9].map(ascii), [0, 1, 3, 3]);
  const text = "aé中😀b";
  const map = byteToUtf16(text);
  // a=1 byte, é=2, 中=3, 😀=4 (two UTF-16 units), b=1
  assert.deepEqual([0, 1, 3, 6, 10, 11, 99].map(map), [0, 1, 2, 3, 5, 6, 6]);
  assert.equal(map(2), 1); // inside é rounds down
});

function harness(request) {
  const timers = [];
  let doc = { toString: () => "one\ntwo" };
  const applied = [];
  const job = folder({
    getDoc: () => doc,
    apply: (r) => applied.push(r),
    request,
    path: () => "main.typ",
    convert: (_source, answer) => answer.ranges.map(({ from, to }) => ({ from, to })),
    setTimer: (fn) => timers.push(fn) && timers.length,
    clearTimer: () => {},
  });
  return { job, timers, applied, change: () => { doc = { toString: () => "other" }; } };
}

test("an answer is applied when the document is unchanged", async () => {
  const h = harness(async () => ({ ranges: [{ from: 3, to: 7, kind: "region" }] }));
  h.job.schedule();
  await h.timers.shift()();
  assert.deepEqual(h.applied, [[{ from: 3, to: 7 }]]);
});

test("a stale answer is dropped and the request rescheduled", async () => {
  let h;
  h = harness(async () => { h.change(); return { ranges: [{ from: 3, to: 7, kind: "region" }] }; });
  h.job.schedule();
  await h.timers.shift()();
  assert.deepEqual(h.applied, []);
  assert.equal(h.timers.length, 1);
});

test("a failing request leaves the answer empty", async () => {
  const h = harness(async () => { throw new Error("no worker"); });
  h.job.schedule();
  await h.timers.shift()();
  assert.deepEqual(h.applied, []);
});

test("stripLabel removes only a trailing label", () => {
  assert.equal(stripLabel("Intro <sec:a>"), "Intro");
  assert.equal(stripLabel("Intro"), "Intro");
  assert.equal(stripLabel("a <b> c"), "a <b> c");
});

test("symbolItems converts byte offsets and kinds, multibyte included", () => {
  const source = "= Été <sec:e>\n#let f(x) = x\n#let a = 1\n";
  const at = (text) => bytes(source.slice(0, source.indexOf(text)));
  const answer = {
    symbols: [
      { name: "Été <sec:e>", kind: "heading", from: 0, to: bytes("= Été <sec:e>"), name_from: 2, name_to: bytes("= Été <sec:e>"), depth: 1 },
      { name: "sec:e", kind: "label", from: at("<sec:e>"), to: at("<sec:e>") + 7, name_from: at("<sec:e>") + 1, name_to: at("<sec:e>") + 6 },
      { name: "f", kind: "function", from: at("let f"), to: at("let f") + bytes("let f(x) = x"), name_from: at("f(x)"), name_to: at("f(x)") + 1 },
      { name: "a", kind: "variable", from: at("let a"), to: at("let a") + 9, name_from: at("a = 1"), name_to: at("a = 1") + 1 },
    ],
  };
  const items = symbolItems(source, answer);
  assert.deepEqual(items.map((i) => i.kind), ["heading", "definition", "definition"]);
  assert.deepEqual(items[0], { kind: "heading", range: [0, 13], level: 1, title: "Été" });
  assert.equal(items[1].name, "f");
  assert.equal(items[1].attrs.type, "function");
  assert.equal(source.slice(...items[1].nameRange), "f");
  assert.equal(source.slice(...items[1].range), "#let f");
  assert.equal(items[2].attrs.type, "variable");
  assert.equal(source.slice(...items[2].nameRange), "a");
  assert.deepEqual(symbolItems(source, null), []);
});

test("headingsOf gives outline entries with lines", () => {
  const source = "= A\ntext\n== B\n";
  const items = symbolItems(source, { symbols: [
    { name: "A", kind: "heading", from: 0, to: 3, name_from: 2, name_to: 3, depth: 1 },
    { name: "B", kind: "heading", from: 9, to: 13, name_from: 12, name_to: 13, depth: 2 },
  ] });
  assert.deepEqual(headingsOf(source, items), [
    { title: "A", level: 1, from: 0, line: 1 },
    { title: "B", level: 2, from: 9, line: 3 },
  ]);
});

test("typstOutline uses module headings only for the same text", () => {
  const stored = { source: "= A\n", headings: [{ title: "Module", level: 1, from: 0, line: 1 }] };
  assert.equal(typstOutline("= A\n", stored)[0].title, "Module");
  assert.equal(typstOutline("= A!\n", stored)[0].title, "A!");
  assert.equal(typstOutline("= A\n", null)[0].title, "A");
});

test("an overlay replaces its kinds and is dropped when the text changes", () => {
  const live = createLiveIndex();
  const source = "= Real\n```\n= Fake\n```\n";
  const snapshot = (text) => ({ texts: { "main.typ": text }, files: ["main.typ"], bibliography: null });
  const headingTitles = () => live.project.file("main.typ").items.filter((i) => i.kind === "heading").map((i) => i.title);
  live.refresh(snapshot(source));
  const before = headingTitles();

  live.setOverlay("main.typ", { kinds: SYMBOL_KINDS, source, items: [{ kind: "heading", range: [0, 6], level: 1, title: "Module" }] });
  assert.equal(live.refresh(snapshot(source)), true);
  assert.deepEqual(headingTitles(), ["Module"]);
  assert.equal(live.refresh(snapshot(source)), false);

  // The text moved on: the overlay's offsets are not trusted.
  assert.equal(live.refresh(snapshot(source + "x")), true);
  assert.deepEqual(headingTitles(), before);

  live.clearOverlay("main.typ");
  assert.equal(live.refresh(snapshot(source)), true);
  assert.deepEqual(headingTitles(), before);
});
