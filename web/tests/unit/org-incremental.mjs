import assert from "node:assert/strict";
import { test } from "node:test";
import { TreeFragment } from "@lezer/common";
import { parseOrg, parseStats, parser } from "../../src/lib/org-language/parser.js";
import { orgTree } from "../../src/lib/source-trees.js";
import { loadOrgSyntaxForTests } from "../helpers/org-syntax.mjs";

await loadOrgSyntaxForTests();

/** About 200 KB: level-1 sections with nested ones, a src block, a table, inline markup. */
function makeDocument(sections = 400) {
  const out = ["#+TITLE: Big\n#+TODO: TODO NEXT | DONE\n\nIntro paragraph with *bold* text.\n\n"];
  for (let i = 0; i < sections; i++) {
    out.push(
      `* ${i % 3 ? "TODO" : "NEXT"} Section ${i} :tag${i}:\n`,
      `Paragraph ${i} with /italic/ and =verbatim= and [[https://example.org/${i}][a link]].\n`,
      "- item one\n- item two\n\n",
      `** Sub ${i}\n#+begin_src js\nconst x = ${i};\n* not a headline in spirit\n#+end_src\n`,
      "| a | b |\n|---+---|\n| 1 | 2 |\n\n",
      "Filler text for size. ".repeat(8) + "\n\n",
    );
  }
  return out.join("");
}

function shape(tree) {
  const out = [];
  tree.iterate({ enter: (n) => void out.push(`${n.name}:${n.from}-${n.to}`) });
  return out.join(" ");
}

/** Parse `after` incrementally from the tree of `before`, one changed range. */
function incremental(before, tree, after, change) {
  const fragments = TreeFragment.applyChanges(TreeFragment.addTree(tree), [change]);
  return parser.parse(after, fragments);
}

const at = (text, needle, from = 0) => {
  const i = text.indexOf(needle, from);
  assert.ok(i >= 0, needle);
  return i;
};

/** The offset of the headline line that holds `needle`. */
const lineOf = (text, needle) => text.lastIndexOf("\n", at(text, needle)) + 1;

const base = makeDocument();
assert.ok(base.length > 150_000);

/** Each edit: [name, function from text to {pos, remove, insert}]. */
const edits = [
  ["type inside a paragraph in the middle", (t) => ({ pos: at(t, "Paragraph 200") + 5, remove: 0, insert: "x" })],
  ["insert a level-1 headline", (t) => ({ pos: lineOf(t, "Section 150 :"), remove: 0, insert: "* New section\nbody\n" })],
  ["insert a deeper headline in the zeroth section", (t) => ({ pos: at(t, "Intro"), remove: 0, insert: "** Early\n" })],
  ["delete a headline line", (t) => {
    const pos = at(t, "\n* ", at(t, "Section 100")) + 1;
    return { pos, remove: t.indexOf("\n", pos) + 1 - pos, insert: "" };
  }],
  ["demote a headline", (t) => ({ pos: at(t, "\n* ", at(t, "Section 90")) + 1, remove: 0, insert: "*" })],
  ["edit a #+TODO line", (t) => ({ pos: at(t, "NEXT |"), remove: 4, insert: "WAIT" })],
  ["edit inside a src block", (t) => ({ pos: at(t, "const x = 250") + 11, remove: 0, insert: "9" })],
  ["break a src block end", (t) => ({ pos: at(t, "#+end_src", at(t, "const x = 120")), remove: 3, insert: "" })],
  ["edit at the very start", () => ({ pos: 0, remove: 0, insert: "z" })],
  ["start with a headline", () => ({ pos: 0, remove: 0, insert: "* First\n" })],
  ["edit at the very end", (t) => ({ pos: t.length, remove: 0, insert: "tail\n" })],
  ["paste several sections", (t) => ({ pos: lineOf(t, "Section 50 :"), remove: 0, insert: makeDocument(5).slice(80) })],
  ["delete a large span", (t) => ({ pos: at(t, "Section 20 "), remove: 30_000, insert: "" })],
  ["join onto a CRLF line", (t) => ({ pos: at(t, "Section 190 "), remove: 0, insert: "\r" })],
];

for (const [name, edit] of edits) {
  test(`incremental parse equals a fresh parse: ${name}`, () => {
    const before = base;
    const tree = parseOrg(before);
    const { pos, remove, insert } = edit(before);
    const after = before.slice(0, pos) + insert + before.slice(pos + remove);
    const change = { fromA: pos, toA: pos + remove, fromB: pos, toB: pos + insert.length };
    const fast = incremental(before, tree, after, change);
    assert.equal(shape(fast), shape(parseOrg(after)));
    assert.equal(fast.length, after.length);
  });
}

test("a chain of edits stays equal to fresh parses", () => {
  let text = base;
  let tree = parseOrg(text);
  for (const [name, edit] of edits) {
    const { pos, remove, insert } = edit(text);
    const next = text.slice(0, pos) + insert + text.slice(pos + remove);
    tree = incremental(text, tree, next, { fromA: pos, toA: pos + remove, fromB: pos, toB: pos + insert.length });
    text = next;
    assert.equal(shape(tree), shape(parseOrg(text)), name);
  }
});

test("a one-character edit in the middle reuses almost every section", () => {
  const tree = parseOrg(base);
  const pos = at(base, "Paragraph 200") + 5;
  const after = base.slice(0, pos) + "x" + base.slice(pos);
  incremental(base, tree, after, { fromA: pos, toA: pos, fromB: pos, toB: pos + 1 });
  assert.ok(parseStats.chunks > 400);
  assert.ok(parseStats.chunks - parseStats.reused <= 3, `${parseStats.chunks - parseStats.reused} of ${parseStats.chunks} reparsed`);
});

test("changing the TODO words parses everything again", () => {
  const tree = parseOrg(base);
  const pos = at(base, "NEXT |");
  const after = base.slice(0, pos) + "WAIT" + base.slice(pos + 4);
  incremental(base, tree, after, { fromA: pos, toA: pos + 4, fromB: pos, toB: pos + 4 });
  assert.equal(parseStats.reused, 0);
});

test("orgTree reuses the previous tree per key", () => {
  const first = orgTree(base, "inc.org");
  const pos = at(base, "Paragraph 300") + 5;
  const after = base.slice(0, pos) + "y" + base.slice(pos);
  const second = orgTree(after, "inc.org");
  assert.notEqual(first, second);
  assert.ok(parseStats.chunks - parseStats.reused <= 3);
  assert.equal(shape(second), shape(parseOrg(after)));
  assert.equal(orgTree(after, "other.org"), second);
});
