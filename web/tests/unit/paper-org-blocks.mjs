import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { loadOrgSyntax } from "../../src/lib/org-language/module.js";
import { orgBlockSource, splitOrgBlocks } from "../../src/lib/paper/org-blocks.js";

// The syntax module must be loaded before the tree can be read.
await loadOrgSyntax(readFileSync(new URL("../../wasm/org.wasm", import.meta.url)));

const text = "#+title: T\n#+author: A\n\n* Heading\n:PROPERTIES:\n:ID: x\n:END:\n\nSome text[fn:1].\nMore text.\n\n** Sub\n- item\n- two\n\n| a | b |\n|---+---|\n\n#+begin_src python\nx = 1\n#+end_src\n\n#+begin_quote\nQuoted.\n#+end_quote\n\n# a comment\n\n$$x$$\n\n[fn:1] Note.\n";

// Sections are containers: their headlines and body blocks come out in order,
// with keywords, drawers, comments and footnote definitions kept as source.
{
  const blocks = splitOrgBlocks(text);
  assert.deepEqual(
    blocks.map((block) => block.kind),
    [
      "Keyword",
      "Keyword",
      "Headline",
      "PropertyDrawer",
      "Paragraph",
      "Headline",
      "List",
      "Table",
      "Block",
      "Block",
      "Comment",
      "Paragraph",
      "FootnoteDefinition",
    ],
  );
  assert.deepEqual(
    blocks.map((block) => block.render),
    [false, false, true, false, true, true, true, true, true, true, false, true, false],
  );
  // Each block is a range of whole lines.
  for (const block of blocks) {
    assert.ok(block.from === 0 || text[block.from - 1] === "\n", `starts a line: ${block.from}`);
    assert.ok(block.to === text.length || text[block.to] === "\n", `ends a line: ${block.to}`);
  }
  assert.equal(text.slice(blocks[2].from, blocks[2].to), "* Heading");
  assert.equal(text.slice(blocks[0].from, blocks[0].to), "#+title: T");
}

// A block's source is its own text, followed by the footnote definitions, so
// that its footnote references resolve when it is rendered alone.
{
  const blocks = splitOrgBlocks(text);
  const paragraph = blocks.find((block) => block.kind === "Paragraph");
  assert.ok(orgBlockSource(text, paragraph, blocks).endsWith("[fn:1] Note."));
  assert.equal(orgBlockSource(text, blocks[2], blocks), "* Heading\n\n[fn:1] Note.");
}

// The last split is remembered by its text, so a repeat call returns the same array.
{
  assert.equal(splitOrgBlocks(text), splitOrgBlocks(text));
}
