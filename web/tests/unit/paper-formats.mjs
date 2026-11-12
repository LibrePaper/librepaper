import assert from "node:assert/strict";
import { paperFormat } from "../../src/lib/paper/formats.js";

// A renderer stand-in: renderBlocks returns what the test gives it and records the arguments.
function fakeRenderer(result) {
  const calls = [];
  return {
    calls,
    render: () => "",
    pageCss: "",
    renderBlocks(source, main) {
      calls.push({ source, main });
      return result;
    },
  };
}

// Line numbers become offsets on whole lines, the last line without a trailing newline included.
{
  const text = "# A\n\nText\nmore";
  const renderer = fakeRenderer([
    { fromLine: 1, toLine: 1, html: "<h1>A</h1>" },
    { fromLine: 3, toLine: 4, html: "<p>Text more</p>" },
  ]);
  const blocks = paperFormat("markdown").renderedBlocks(text, renderer, "notes/paper.md");
  assert.deepEqual(renderer.calls, [{ source: text, main: "notes/paper.md" }]);
  assert.deepEqual(blocks, [
    { from: 0, to: 3, kind: "Block", render: true, html: "<h1>A</h1>" },
    { from: 5, to: text.length, kind: "Block", render: true, html: "<p>Text more</p>" },
  ]);
  assert.equal(text.slice(blocks[1].from, blocks[1].to), "Text\nmore");
}

// CRLF breaks count as one line break, and a trailing newline leaves an empty last line.
{
  const text = "one\r\ntwo\n";
  const renderer = fakeRenderer([{ fromLine: 2, toLine: 2, html: "<p>two</p>" }]);
  const blocks = paperFormat("markdown").renderedBlocks(text, renderer, "a.md");
  assert.equal(blocks.length, 1);
  assert.equal(text.slice(blocks[0].from, blocks[0].to), "two");
}

// A null from the renderer means the adapter falls back: null for Markdown, nothing drawn for Quarto.
{
  const renderer = fakeRenderer(null);
  assert.equal(paperFormat("markdown").renderedBlocks("# A", renderer, "a.md"), null);
  assert.equal(paperFormat("quarto").renderedBlocks("# A", renderer, "a.qmd"), null);
  assert.equal(paperFormat("quarto").splitBlocks("# A"), null);
}

// Quarto is a paper format, with the Markdown module.
{
  assert.equal(paperFormat("quarto").module, "markdown");
  assert.equal(paperFormat("bogus"), null);
}
