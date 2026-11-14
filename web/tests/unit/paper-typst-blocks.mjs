import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { loadTypstSyntax } from "../../src/lib/typst-language/module.js";
import { splitTypstBlocks, typstBlockSource } from "../../src/lib/paper/typst-blocks.js";
import { createTypstPaperRenderer } from "../../src/lib/paper/typst-renderer.js";

const syntaxWasm = new URL("../../wasm/typst-syntax.wasm", import.meta.url);
const typstWasm = new URL("../../wasm/typst.wasm", import.meta.url);

if (!existsSync(syntaxWasm)) {
  console.log("typst-syntax.wasm is missing; skipping the Typst paper tests");
} else {
  // Without the syntax module there is no tree, and so no blocks.
  assert.equal(splitTypstBlocks("= A\n"), null);
  await loadTypstSyntax(new Uint8Array(readFileSync(syntaxWasm)));

  const text = [
    '#set heading(numbering: "1.")',
    "#set math.equation(numbering: \"(1)\")",
    "#let note(body) = text(style: \"italic\", body)",
    "// a comment",
    "",
    "= Intro <sec:intro>",
    "",
    "See @eq-x and $a$.",
    "",
    "$ a + b $ <eq-x>",
    "",
    "#figure(table(columns: 1, [x]), caption: [T]) <tab-t>",
    "",
    "= Next",
    "",
    "Back to @sec:intro, @eq-x and @tab-t.",
    "",
  ].join("\n");

  // Only the rule itself is replayed: text sharing its line is not repeated in later blocks.
  {
    const shared = "#set text(size: 11pt) Hello\n\nWorld.\n";
    const blocks = splitTypstBlocks(shared);
    const last = blocks.at(-1);
    const source = typstBlockSource(shared, last, blocks);
    assert.ok(source.includes("#set text(size: 11pt)"), source);
    assert.equal(source.split("Hello").length - 1, 0, source);
  }

  // Preamble rules and comments stay as source; everything else is drawn, on whole lines.
  {
    const blocks = splitTypstBlocks(text);
    assert.deepEqual(
      blocks.map((block) => [block.kind, text.slice(block.from, block.to), block.render]),
      [
        ["SetRule", '#set heading(numbering: "1.")', false],
        ["SetRule", '#set math.equation(numbering: "(1)")', false],
        ["LetBinding", '#let note(body) = text(style: "italic", body)', false],
        ["Comment", "// a comment", false],
        ["Heading", "= Intro <sec:intro>", true],
        ["Text", "See @eq-x and $a$.", true],
        ["Equation", "$ a + b $ <eq-x>", true],
        ["FuncCall", "#figure(table(columns: 1, [x]), caption: [T]) <tab-t>", true],
        ["Heading", "= Next", true],
        ["Text", "Back to @sec:intro, @eq-x and @tab-t.", true],
      ],
    );
    for (const block of blocks) {
      assert.ok(block.from === 0 || text[block.from - 1] === "\n");
      assert.ok(block.to === text.length || text[block.to] === "\n");
    }
  }

  // The last split is remembered by its text.
  assert.equal(splitTypstBlocks(text), splitTypstBlocks(text));

  // Two blocks on one line become one, and stay source if either part does.
  {
    const blocks = splitTypstBlocks("#set text(size: 11pt) Hello\n");
    assert.equal(blocks.length, 1);
    assert.equal(blocks[0].render, false);
  }

  // A later heading is compiled with the preamble, the references and the numbering before it.
  {
    const blocks = splitTypstBlocks(text);
    const next = blocks.filter((block) => block.kind === "Heading")[1];
    const source = typstBlockSource(text, next, blocks);
    assert.ok(source.startsWith('#set heading(numbering: "1.")\n'));
    assert.ok(source.includes("#let note(body)"));
    assert.ok(!source.includes("// a comment"));
    assert.ok(source.includes('#let __lp_refs = ("sec:intro": "Section 1", "eq-x": "Equation 1", "tab-t": "Table 1")'));
    assert.ok(source.includes("#show ref:"));
    assert.ok(source.includes("#counter(heading).update((1,))"));
    assert.ok(source.includes("#counter(math.equation).update(1)"));
    assert.ok(source.includes("#counter(figure.where(kind: table)).update(1)"));
    assert.ok(source.endsWith("= Next"));
    // The first heading has nothing before it to count.
    const first = typstBlockSource(text, blocks.find((block) => block.kind === "Heading"), blocks);
    assert.ok(!first.includes("#counter"));
    assert.ok(first.endsWith("= Intro <sec:intro>"));
  }

  // An include is a block of its own: not drawn, and never replayed in later blocks.
  {
    const included = '#set text(size: 11pt)\n\n= Title\n\n#include "part.typ"\n\nAfter the include.\n';
    const blocks = splitTypstBlocks(included);
    const include = blocks.find((block) => block.kind === "ModuleInclude");
    assert.ok(include, JSON.stringify(blocks));
    assert.equal(include.render, false);
    assert.equal(include.rules, undefined);
    assert.equal(included.slice(include.from, include.to), '#include "part.typ"');
    const after = blocks.find((block) => block.kind === "Text");
    const source = typstBlockSource(included, after, blocks);
    assert.ok(source.includes("#set text"), source);
    assert.ok(!source.includes("#include"), source);
  }

  if (!existsSync(typstWasm)) {
    console.log("typst.wasm is missing; skipping the Typst paper renderer tests");
  } else {
    const renderer = await createTypstPaperRenderer(readFileSync(typstWasm));
    const html = renderer.render("= Hello there\n\nSome text with $x^2$ in it.");
    assert.match(html, /Hello there/);
    assert.match(html, /<math/);
    assert.doesNotMatch(html, /<body|<html/);
    assert.ok(renderer.mathCss.length > 0);
    assert.equal(renderer.render("= Same"), renderer.render("= Same"));
    // A source that does not compile draws nothing.
    assert.equal(renderer.render("#unknown-function()"), "");

    // A reference alone in its block reads the text of its target.
    const blocks = splitTypstBlocks(text);
    const last = blocks.at(-1);
    const rendered = renderer.render(typstBlockSource(text, last, blocks));
    assert.match(rendered, /Section 1/);
    assert.match(rendered, /Equation 1/);
    assert.match(rendered, /Table 1/);
  }
}
