import assert from "node:assert/strict";
import { blockSource, splitBlocks } from "../../src/lib/paper/blocks.js";

// Every kind of top-level block, each on whole lines, with the render flag the
// editor relies on: definitions and front matter stay as source.
{
  const text = [
    "---",
    "title: Paper",
    "---",
    "",
    "# Heading",
    "",
    "Some *text*",
    "continues here.",
    "",
    "- one",
    "- two",
    "",
    "| a | b |",
    "|---|---|",
    "| 1 | 2 |",
    "",
    "```",
    "code",
    "```",
    "",
    "[ref]: https://example.com",
    "",
    "[^1]: The note.",
    "",
  ].join("\n");
  const blocks = splitBlocks(text);
  assert.deepEqual(
    blocks.map((block) => [block.kind, text.slice(block.from, block.to), block.render]),
    [
      ["FrontMatter", "---\ntitle: Paper\n---", false],
      ["ATXHeading1", "# Heading", true],
      ["Paragraph", "Some *text*\ncontinues here.", true],
      ["BulletList", "- one\n- two", true],
      ["Table", "| a | b |\n|---|---|\n| 1 | 2 |", true],
      ["FencedCode", "```\ncode\n```", true],
      ["LinkReference", "[ref]: https://example.com", false],
      ["FootnoteDefinition", "[^1]: The note.", false],
    ],
  );
}

// A block's boundaries are whole lines: from the start of its first line to the
// line break that ends its last one, or the end of the text.
{
  const text = "# A\n\nB";
  const blocks = splitBlocks(text);
  assert.equal(blocks.length, 2);
  assert.equal(blocks[1].from, 5);
  assert.equal(blocks[1].to, text.length);
}

// An empty source has no blocks.
{
  assert.deepEqual(splitBlocks(""), []);
}

// The last split is remembered by its text, so a repeat call returns the same array.
{
  const text = "# Same\n\nText";
  assert.equal(splitBlocks(text), splitBlocks(text));
}

// A block's source is its own text, followed by the document's definitions,
// so that its references resolve when it is rendered alone.
{
  const text = "# A\n\nSome *text*\ncontinues here.\n\n[ref]: https://example.com\n\n[^1]: The note.\n";
  const blocks = splitBlocks(text);
  const paragraph = blocks.find((block) => block.kind === "Paragraph");
  assert.equal(
    blockSource(text, paragraph, blocks),
    "Some *text*\ncontinues here.\n\n[ref]: https://example.com\n\n[^1]: The note.",
  );
}

// A block with no definitions in the document is its own text alone.
{
  const text = "# A\n\nB";
  const blocks = splitBlocks(text);
  assert.equal(blockSource(text, blocks[0], blocks), "# A");
}
