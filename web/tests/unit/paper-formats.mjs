import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { paperFormat } from "../../src/lib/paper/formats.js";
import { loadTypstSyntax } from "../../src/lib/typst-language/module.js";

// A renderer stand-in: renderBlocks returns what the test gives it and records the arguments.
function fakeRenderer(result) {
  const calls = [];
  return {
    calls,
    render: () => "",
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

// Only Typst frames its blocks and draws a header; the other formats have neither hook.
{
  assert.equal(paperFormat("markdown").header, undefined);
  assert.equal(paperFormat("org").framePrefix, undefined);
}

// The Typst header is the frame's output under the preamble pill. It needs the syntax module to split blocks.
const typstSyntaxWasm = new URL("../../wasm/typst-syntax.wasm", import.meta.url);
if (!existsSync(typstSyntaxWasm)) {
  console.log("typst-syntax.wasm is missing; skipping the Typst header tests");
} else {
  await loadTypstSyntax(new Uint8Array(readFileSync(typstSyntaxWasm)));

  // A renderer stand-in: frame returns what the test gives it and records the prefixes it was asked for.
  function fakeFrameRenderer(frame) {
    const calls = [];
    return {
      calls,
      render: () => "",
      frame(prefix) {
        calls.push(prefix);
        return frame;
      },
    };
  }

  const doc = ["#let article(title: none, body) = { title; body }", "#show: article.with(title: [T])", "", "First paragraph.", ""].join("\n");
  const typst = paperFormat("typst");
  const blocks = typst.splitBlocks(doc);
  const folds = [{ from: 0, to: doc.indexOf("\n\nFirst"), kind: "preamble", detail: "#let · #show", others: "#let · #show" }];

  // The frame's before-text is drawn as the title block, from the start of the preamble to its end.
  {
    const renderer = fakeFrameRenderer({ before: "<p>T</p>", after: "" });
    const header = typst.header(doc, blocks, folds, renderer, null, "main.typ");
    assert.deepEqual(header, {
      from: 0,
      to: folds[0].to,
      kind: "TitleBlock",
      render: true,
      html: '<header class="title-block"><p>T</p></header>',
    });
    assert.equal(renderer.calls.length, 1);
    assert.ok(renderer.calls[0].startsWith("#let article"), renderer.calls[0]);
    assert.ok(renderer.calls[0].includes("#show: article.with(title: [T])"), renderer.calls[0]);
  }

  // A frame that writes nothing shown draws no header.
  for (const frame of [{ before: "", after: "" }, { before: "<div></div>", after: "" }, null]) {
    assert.equal(typst.header(doc, blocks, folds, fakeFrameRenderer(frame), null, "main.typ"), null);
  }

  // An image alone is shown, so it draws a header.
  {
    const renderer = fakeFrameRenderer({ before: '<img src="x.png">', after: "" });
    assert.notEqual(typst.header(doc, blocks, folds, renderer, null, "main.typ"), null);
  }

  // No preamble fold, or a renderer without frame: no header.
  {
    const renderer = fakeFrameRenderer({ before: "<p>T</p>", after: "" });
    assert.equal(typst.header(doc, blocks, [], renderer, null, "main.typ"), null);
    assert.equal(renderer.calls.length, 0);
    assert.equal(typst.header(doc, blocks, folds, { render: () => "" }, null, "main.typ"), null);
  }

  // A document without a whole-document show rule has no frame, so the renderer is not asked.
  {
    const plain = ["#set text(11pt)", "", "Text.", ""].join("\n");
    const plainBlocks = typst.splitBlocks(plain);
    const plainFolds = [{ from: 0, to: plain.indexOf("\n"), kind: "preamble", detail: "#set", others: "#set" }];
    const renderer = fakeFrameRenderer({ before: "<p>T</p>", after: "" });
    assert.equal(typst.header(plain, plainBlocks, plainFolds, renderer, null, "main.typ"), null);
    assert.equal(renderer.calls.length, 0);
  }
}
