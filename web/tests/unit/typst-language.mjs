import assert from "node:assert/strict";
import test from "node:test";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { EditorState } from "@codemirror/state";
import { foldable, syntaxTree } from "@codemirror/language";
import { highlightTree, tags } from "@lezer/highlight";
import { loadTypstSyntax, typstSyntax, changedRange } from "../../src/lib/typst-language/module.js";
import { typstSupport } from "../../src/lib/typst-language/index.js";

const wasm = process.env.LIBREPAPER_TYPST_SYNTAX_WASM || fileURLToPath(new URL("../../wasm/typst-syntax.wasm", import.meta.url));

test("changedRange finds the one edited span and keeps surrogate pairs whole", () => {
  assert.deepEqual(changedRange("abcdef", "abXdef"), { from: 2, to: 3, insert: "X" });
  assert.deepEqual(changedRange("abc", "abcd"), { from: 3, to: 3, insert: "d" });
  const range = changedRange("a😀b", "a😁b");
  assert.deepEqual(range, { from: 1, to: 3, insert: "😁" });
});

if (!existsSync(wasm)) {
  test("typst-syntax.wasm is missing", (t) => t.skip(`${wasm} not found; set LIBREPAPER_TYPST_SYNTAX_WASM`));
} else {
  const api = await loadTypstSyntax(new Uint8Array(readFileSync(wasm)));
  const names = (tree) => {
    const found = [];
    tree.iterate({ enter: (node) => { found.push(node.name); } });
    return found;
  };
  const doc = "= Intro\nSome *bold* text.\n\n#let f(x) = {\n  x + 1\n}\n\n$ a + b $\n\n== Sub\nmore\n= Next\n";

  test("the module loads and is cached", () => {
    assert.ok(api, "typst-syntax.wasm did not load");
    assert.equal(typstSyntax(), api);
  });

  test("a small document has the expected tree", () => {
    const tree = api.tree(doc);
    assert.equal(tree.length, doc.length);
    const found = names(tree);
    for (const name of ["Document", "Markup", "Heading", "HeadingMarker", "Strong", "LetBinding", "CodeBlock", "Equation", "Math"]) {
      assert.ok(found.includes(name), `no ${name} in ${found.join(" ")}`);
    }
  });

  test("an edit through a key gives the tree a fresh parse gives", () => {
    const edited = doc.replace("*bold*", "*bolder* 😀 é").replace("x + 1", "x + 2");
    api.tree(doc, "a");
    const incremental = api.tree(edited, "a");
    const fresh = api.tree(edited, "fresh");
    assert.equal(incremental.toString(), fresh.toString());
    const keyless = api.tree(doc);
    assert.equal(api.tree(edited).toString(), fresh.toString());
    assert.equal(keyless.length, doc.length);
  });

  test("the same text gives the same tree", () => {
    assert.equal(api.tree("= Same\n"), api.tree("= Same\n", "other"));
  });

  test("offsets are UTF-16", () => {
    const text = "😀 *b*\n";
    let strong = null;
    api.tree(text).iterate({ enter: (node) => { if (node.name === "Strong") strong = [node.from, node.to]; } });
    assert.deepEqual(strong, [3, 6]);
  });

  test("tags resolve for headings, strong, raw and keywords", () => {
    const text = "= Intro\n*bold* `raw` #let x = 1\n";
    const seen = new Set();
    highlightTree(api.tree(text), { style: (set) => (set.includes(tags.heading) ? "heading" : set.includes(tags.strong) ? "strong" : set.includes(tags.monospace) ? "monospace" : set.includes(tags.definitionKeyword) ? "keyword" : set.includes(tags.number) ? "number" : null) },
      (from, to, cls) => seen.add(cls));
    for (const cls of ["heading", "strong", "monospace", "keyword", "number"]) assert.ok(seen.has(cls), `no ${cls} in ${[...seen]}`);
  });

  test("code blocks and heading sections fold", () => {
    const support = typstSupport(api);
    const state = EditorState.create({ doc, extensions: support });
    assert.ok(syntaxTree(state).length > 0);
    const letLine = state.doc.line(4);
    const block = foldable(state, letLine.from, letLine.to);
    assert.equal(state.doc.sliceString(block.from, block.to), "\n  x + 1\n");
    const heading = state.doc.line(1);
    const section = foldable(state, heading.from, heading.to);
    assert.equal(section.from, heading.to);
    assert.equal(state.doc.sliceString(section.from, section.to).trimEnd().endsWith("more"), true);
    // A deeper heading stops at the next heading of its level or above.
    const sub = state.doc.line(10);
    const subSection = foldable(state, sub.from, sub.to);
    assert.equal(state.doc.sliceString(subSection.from, subSection.to), "\nmore");
  });
}
