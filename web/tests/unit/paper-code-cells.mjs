import assert from "node:assert/strict";
import { withCodeCells } from "../../src/lib/paper/code-cells.js";
import { paperFormat } from "../../src/lib/paper/formats.js";
import { m } from "../../src/lib/paraglide/messages.js";

// A renderer stand-in: the body of a fenced block, escaped, inside a <pre>.
const renderer = {
  calls: [],
  render(markdown) {
    this.calls.push(markdown);
    const body = markdown.split("\n").slice(1, -2).join("\n");
    return `<pre><code>${body.replace(/&/g, "&amp;").replace(/</g, "&lt;")}</code></pre>`;
  },
};

// A block for each line range of the text, drawn as a paragraph, as the module would render prose.
function paragraph(text, fromLine, toLine, html = "<p>x</p>") {
  const lines = text.split("\n");
  const from = lines.slice(0, fromLine).join("\n").length + (fromLine > 0 ? 1 : 0);
  const to = lines.slice(0, toLine + 1).join("\n").length;
  return { from, to, kind: "Block", render: true, html };
}

// The offset of a line's start, by its index.
function lineAt(text, index) {
  return text.split("\n").slice(0, index).join("\n").length + (index > 0 ? 1 : 0);
}

// An echo:false chunk with no block of its own becomes a card: no options, the note, and the code.
{
  const text = ["Intro", "", "```{r}", "#| echo: false", "x <- 1", "```", "", "After"].join("\n");
  const blocks = [paragraph(text, 0, 0), paragraph(text, 7, 7, "<p>After</p>")];
  const result = withCodeCells(text, blocks, renderer, "a.qmd");
  assert.deepEqual(result.map((block) => block.kind), ["Block", "CodeCell", "Block"]);
  const card = result[1];
  assert.equal(card.from, lineAt(text, 2));
  assert.equal(text.slice(card.from, card.to), "```{r}\n#| echo: false\nx <- 1\n```");
  assert.equal(card.render, true);
  assert.ok(card.html.startsWith('<div class="code-filename lp-paper-cell-head lp-paper-cell-quiet lp-paper-cell-collapsed">'));
  assert.ok(card.html.includes('<span class="lp-paper-cell-lang">r</span><span class="lp-paper-cell-note">' + m.codecells_code_hidden() + '</span><span class="lp-paper-cell-lines">' + m.codecells_lines({ count: 1 }) + "</span></div>"));
  assert.ok(!card.html.includes("<pre"));
  assert.ok(!card.html.includes("x &lt;- 1"));
  assert.ok(!card.html.includes("#|"));
  assert.ok(!card.html.includes("```"));
}

// An echo:true chunk's own block is replaced: the card has no note and the code is drawn without options.
{
  const text = "```{python}\n#| label: plot\nprint(1)\n```\n";
  const blocks = [{ from: 0, to: text.length - 1, kind: "Block", render: true, html: "<pre>fence</pre>" }];
  const result = withCodeCells(text, blocks, renderer, "a.qmd");
  assert.equal(result.length, 1);
  assert.equal(result[0].kind, "CodeCell");
  assert.equal(result[0].to, text.length - 1);
  assert.ok(result[0].html.startsWith('<div class="code-filename lp-paper-cell-head">'));
  assert.ok(result[0].html.includes('<span class="lp-paper-cell-label">plot</span>'));
  assert.ok(!result[0].html.includes("lp-paper-cell-note"));
  assert.ok(!result[0].html.includes("#|"));
}

// Front matter execute defaults apply, and the cell's own option overrides them.
{
  const text = "---\nexecute:\n  echo: false\n---\n\n```{r}\nx\n```\n\n```{r}\n#| echo: true\ny\n```\n";
  const result = withCodeCells(text, [], renderer, "a.qmd");
  assert.equal(result.length, 2);
  assert.ok(result[0].html.includes(m.codecells_code_hidden()));
  assert.ok(!result[1].html.includes("lp-paper-cell-note"));
  assert.ok(!result[1].html.includes("lp-paper-cell-quiet"));
}

// include: false and eval: false give their notes, and include wins over echo.
{
  const text = "```{r}\n#| include: false\nx\n```\n\n```{r}\n#| eval: false\ny\n```\n\n```{r}\n#| include: false\n#| echo: false\nz\n```\n";
  const result = withCodeCells(text, [], renderer, "a.qmd");
  assert.ok(result[0].html.includes(m.codecells_not_in_output()));
  assert.ok(result[1].html.includes(m.codecells_not_run()));
  assert.ok(result[2].html.includes(m.codecells_not_in_output()));
  assert.ok(!result[2].html.includes(m.codecells_code_hidden()));
}

// A block that runs over a chunk's edge leaves the chunk alone.
{
  const text = "Para line\n```{r}\nx\n```\n";
  const blocks = [{ from: 0, to: text.indexOf("x") + 1, kind: "Block", render: true, html: "<p>straddle</p>" }];
  const result = withCodeCells(text, blocks, renderer, "a.qmd");
  assert.deepEqual(result, blocks);
}

// A plain ```python fence is not an executable chunk and is untouched.
{
  const text = "```python\nprint(1)\n```\n";
  const blocks = [{ from: 0, to: text.length - 1, kind: "Block", render: true, html: "<pre>plain</pre>" }];
  assert.deepEqual(withCodeCells(text, blocks, renderer, "a.md"), blocks);
}

// The language is shown as written, and the label is escaped.
{
  const text = "```{R}\n#| label: a<b>&\n#| echo: false\nx\n```\n";
  const [card] = withCodeCells(text, [], renderer, "a.qmd");
  assert.ok(card.html.includes('<span class="lp-paper-cell-lang">R</span><span class="lp-paper-cell-label">a&lt;b&gt;&amp;</span>'));
}

// A plain chunk is drawn open: its code is rendered and the card is not collapsed.
{
  const text = "```{r}\nx <- 1\ny <- 2\n```\n";
  const [card] = withCodeCells(text, [], renderer, "a.qmd");
  assert.ok(card.html.includes("<pre>"));
  assert.ok(!card.html.includes("lp-paper-cell-collapsed"));
  assert.ok(!card.html.includes("lp-paper-cell-lines"));
}

// code-fold: true in the front matter collapses the chunks; a chunk's code-fold: show overrides it.
{
  const text = "---\ncode-fold: true\n---\n\n```{r}\nx\n```\n\n```{r}\n#| code-fold: show\ny\nz\n```\n";
  const result = withCodeCells(text, [], renderer, "a.qmd");
  assert.equal(result.length, 2);
  assert.ok(result[0].html.includes("lp-paper-cell-collapsed"));
  assert.ok(!result[0].html.includes("<pre"));
  assert.ok(result[0].html.includes('<span class="lp-paper-cell-lines">' + m.codecells_lines({ count: 1 }) + "</span>"));
  assert.ok(!result[1].html.includes("lp-paper-cell-collapsed"));
  assert.ok(result[1].html.includes("<pre>"));
}

// format.html.code-fold is the document default when the front matter has no top-level key.
{
  const text = "---\nformat:\n  html:\n    code-fold: \"true\"\n---\n\n```{r}\nx\n```\n";
  const [card] = withCodeCells(text, [], renderer, "a.qmd");
  assert.ok(card.html.includes("lp-paper-cell-collapsed"));
}

// eval: false alone leaves the code shown, so the card stays open.
{
  const text = "```{r}\n#| eval: false\nx\n```\n";
  const [card] = withCodeCells(text, [], renderer, "a.qmd");
  assert.ok(card.html.includes("<pre>"));
  assert.ok(!card.html.includes("lp-paper-cell-collapsed"));
  assert.ok(card.html.includes(m.codecells_not_run()));
}

// Null stays null; the quarto adapter draws its cards through renderedBlocks.
{
  assert.equal(withCodeCells("x", null, renderer, "a.qmd"), null);
  const text = "Text\n\n```{r}\n#| echo: false\nx\n```\n";
  const drawing = {
    render: renderer.render.bind(renderer),
    renderBlocks: () => [{ fromLine: 1, toLine: 1, html: "<p>Text</p>" }],
  };
  const blocks = paperFormat("quarto").renderedBlocks(text, drawing, "a.qmd");
  assert.deepEqual(blocks.map((block) => block.kind), ["Block", "CodeCell"]);
  assert.equal(text.slice(blocks[1].from, blocks[1].to), "```{r}\n#| echo: false\nx\n```");
}
