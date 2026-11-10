// The Org tree from org.wasm: a parse that reuses sections equals a fresh parse,
// and a change to the TODO words reparses every section.
import assert from "node:assert/strict";
import { test } from "node:test";
import { TreeFragment } from "@lezer/common";
import { parseStats } from "../../src/lib/org-language/module.js";
import { loadOrgSyntaxForTests } from "../helpers/org-syntax.mjs";

const api = await loadOrgSyntaxForTests();

const shape = (tree) => {
  const out = [];
  tree.iterate({ enter: (n) => void out.push(`${n.name}:${n.from}-${n.to}`) });
  return out.join(" ");
};

const documents = {
  plain: "Just a paragraph.\n\nAnother with *bold*, /italic/ and =verbatim=.\n",
  headlines: "#+TITLE: T\n#+TODO: TODO NEXT | DONE\n\n* TODO One :a:b:\nText [[https://example.org][link]].\n** NEXT Two\n- item\n- [ ] box\n* DONE Three\n| a | b |\n|---+---|\n| 1 | 2 |\n",
  blocks: "* H\n#+begin_src js\nconst x = 1;\n* not a headline\n#+end_src\n\n#+begin_quote\nQuoted *text*\n#+end_quote\n",
  inline: "* Math\nInline $x^2$ and \\(y\\), cite [cite:@smith2020;@doe], footnote[fn:1].\n\n[fn:1] A note.\n",
  unicode: "* Größe \u{1F600}\nText with é and \u{1F600} inside *bold*.\n",
};

for (const [name, text] of Object.entries(documents)) {
  test(`a parse that reuses every section equals a fresh parse: ${name}`, () => {
    const tree = api.parse(text);
    const again = api.parse(text, TreeFragment.addTree(tree));
    assert.equal(shape(again), shape(api.parse(text)));
  });
}

function sections(count) {
  let text = "#+TODO: TODO NEXT | DONE\n\nIntro.\n\n";
  for (let i = 0; i < count; i++) text += `* TODO Section ${i}\nParagraph ${i} with *bold* and [[https://example.org/${i}][link]].\n\n- one\n- two\n\n`;
  return text;
}

test("an edit reparsed with fragments equals a fresh parse", () => {
  const before = sections(30);
  const tree = api.parse(before);
  const at = before.indexOf("Paragraph 12") + 4;
  const after = before.slice(0, at) + "X" + before.slice(at);
  const fragments = TreeFragment.applyChanges(TreeFragment.addTree(tree), [{ fromA: at, toA: at, fromB: at, toB: at + 1 }]);
  const fast = api.parse(after, fragments);
  assert.equal(shape(fast), shape(api.parse(after)));
});

test("a change to the TODO words reparses every section", () => {
  const before = sections(5);
  const tree = api.parse(before);
  const after = before.replace("#+TODO: TODO NEXT | DONE", "#+TODO: TODO Section | DONE");
  const change = { fromA: 8, toA: 22, fromB: 8, toB: 28 };
  const fragments = TreeFragment.applyChanges(TreeFragment.addTree(tree), [change]);
  const fast = api.parse(after, fragments);
  assert.equal(parseStats.reused, 0);
  assert.equal(shape(fast), shape(api.parse(after)));
});

test("an empty text gives an empty document", () => {
  const tree = api.parse("");
  assert.equal(tree.length, 0);
  assert.equal(tree.type.name, "Document");
});
