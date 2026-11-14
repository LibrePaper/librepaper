import assert from "node:assert/strict";
import fs from "node:fs";
import { cellFingerprint, parseQuarto } from "../../src/lib/engines/quarto.js";
import { splitResults } from "../../src/lib/quarto-results.js";

const read = (name) => fs.readFileSync(new URL(`../fixtures/quarto-results/${name}`, import.meta.url), "utf8");
const html = read("u.html");
const qmd = read("u.qmd");

const result = await splitResults(html, qmd, { path: "paper.qmd" });
assert.equal(result.ok, true);
const { record } = result;
assert.equal(record.version, 1);
assert.equal(record.path, "paper.qmd");
// Six source cells; the setup cell has `include: false` and prints nothing.
assert.equal(parseQuarto(qmd).cells.length, 6);
assert.equal(record.cells.length, 5);
assert.deepEqual(record.cells.map((cell) => cell.label), ["tbl-head", null, "fig-plot", null, null]);

const expected = parseQuarto(qmd, { path: "paper.qmd" }).cells.filter((cell) => cell.label !== "setup");
assert.deepEqual(record.cells.map((cell) => cell.fingerprint), await Promise.all(expected.map(cellFingerprint)));
for (const cell of record.cells) assert.match(cell.fingerprint, /^[0-9a-f]{64}$/);

for (const cell of record.cells) {
  assert.doesNotMatch(cell.html, /cell-code|sourceCode|code-fold|code-copy|\n|\r/);
  assert.match(cell.html, /^<(div|details)[^>]*class="[^"]*\bcell\b/);
}
assert.match(record.cells[0].html, /Mazda RX4 Wag/);
assert.match(record.cells[0].html, /<figcaption/, "a tbl- float keeps its caption");
assert.match(record.cells[2].html, /<img [^>]*src="data:image\/png;base64,AAAA"/);
// Newlines inside <pre> survive as character references, blank lines included.
assert.match(record.cells[3].html, /hidden code&#10;&#10;with blank line/);
assert.match(record.cells[1].html, /\[1\] 1 2 3/);
assert.match(record.cells[4].html, /Min\. 1st Qu\./);
// Neither the echoed `summary(1:10)` nor the fold summary survives.
assert.doesNotMatch(record.cells[4].html, /summary\(|<summary|<details/);

// Event handlers and scripts do not travel.
const hostile = `<main id="quarto-document-content">
<script>alert(1)</script>
<div class="cell" onclick="steal()">
<div class="cell-output" onmouseover='x()' data-ok="1"><pre><code>a
b</code></pre><script>evil()</script></div>
</div></main>`;
const clean = await splitResults(hostile, "```{r}\n1\n```\n");
assert.equal(clean.ok, true);
assert.doesNotMatch(clean.record.cells[0].html, /onclick|onmouseover|script|alert|evil/i);
assert.match(clean.record.cells[0].html, /data-ok="1"/);
assert.match(clean.record.cells[0].html, /a&#10;b/);
assert.equal(clean.record.path, "main.qmd");

// Cells nested in another cell are not counted twice.
const nested = `<main id="quarto-document-content"><div class="cell"><div class="cell"><p>in</p></div></div></main>`;
assert.equal((await splitResults(nested, "```{r}\n1\n```\n")).ok, true);

// Without the quarto main element the whole page is searched.
const bare = `<body><div class="cell"><p>x</p></div></body>`;
assert.equal((await splitResults(bare, "```{r}\n1\n```\n")).ok, true);

// Front matter defaults switch cells off too, and a cell option wins.
const defaulted = "---\nexecute:\n  include: false\n---\n\n```{r}\n1\n```\n\n```{r}\n#| include: true\n2\n```\n";
const one = `<main id="quarto-document-content"><div class="cell"><p>two</p></div></main>`;
const defaultedResult = await splitResults(one, defaulted);
assert.equal(defaultedResult.ok, true);
assert.equal(defaultedResult.record.cells.length, 1);

assert.deepEqual(await splitResults(html, qmd.replace(/```\{r\}\nx <- 1:3[\s\S]*?```\n/, "")), { ok: false, reason: "count-mismatch" });
assert.deepEqual(await splitResults("<main id=\"quarto-document-content\"><p>none</p></main>", qmd), { ok: false, reason: "no-cells" });
assert.deepEqual(await splitResults(html, "No code here.\n"), { ok: false, reason: "no-cells" });
assert.deepEqual(await splitResults("<main id=\"quarto-document-content\"><div class=\"cell\"><p>open", qmd), { ok: false, reason: "parse" });

// Cells in included files count, in document order, like the renderer splices them.
{
  const { expandedCells, quartoSidecar } = await import("../../src/lib/engines/quarto.js");
  const cell = (n) => `\`\`\`{r}\n${n}\n\`\`\`\n`;
  const files = {
    "part.qmd": `---\ntitle: part\n---\n\nText\n\n${cell("part")}`,
    "chap/a.qmd": `${cell("a")}\n{{< include b.qmd >}}\n`,
    "chap/b.qmd": cell("b"),
    "loop1.qmd": `${cell("l1")}\n{{< include loop2.qmd >}}\n`,
    "loop2.qmd": `${cell("l2")}\n{{< include loop1.qmd >}}\n`,
  };
  const main = `${cell("m1")}\n{{< include part.qmd >}}\n\n${cell("m2")}`;
  const cellsHtml = (n) => `<main id="quarto-document-content">${Array.from({ length: n }, (_, i) => `<div class="cell"><p>${i}</p></div>`).join("")}</main>`;

  const split = await splitResults(cellsHtml(3), main, { path: "main.qmd", files });
  assert.equal(split.ok, true);
  assert.deepEqual(split.record.cells.map((c) => c.path ?? null), [null, "part.qmd", null]);
  assert.equal((await splitResults(cellsHtml(2), main, { path: "main.qmd", files })).reason, "count-mismatch");
  assert.equal((await splitResults(cellsHtml(2), main, { path: "main.qmd" })).ok, true, "without files the include contributes nothing");

  const sidecar = await quartoSidecar(main, { path: "main.qmd", files, results: split.record });
  assert.deepEqual(sidecar.outputs.map((o) => [o.line, o.path ?? null, o.stale]), [[1, null, false], [7, "part.qmd", false], [7, null, false]]);

  const nested = expandedCells("{{< include chap/a.qmd >}}\n", { path: "main.qmd", files });
  assert.deepEqual(nested.map((c) => c.file), ["chap/a.qmd", "chap/b.qmd"]);

  const fenced = expandedCells("```\n{{< include part.qmd >}}\n```\n", { path: "main.qmd", files });
  assert.equal(fenced.length, 0);

  const cyclic = expandedCells("{{< include loop1.qmd >}}\n", { path: "main.qmd", files });
  assert.deepEqual(cyclic.map((c) => c.file), ["loop1.qmd", "loop2.qmd"]);

  assert.equal(expandedCells("{{< include ../x.qmd >}}\n", { path: "main.qmd", files: { "../x.qmd": cell("x") } }).length, 1, "falls back to the name as written");
  assert.equal(expandedCells("{{< include /part.qmd >}}\n", { path: "main.qmd", files }).length, 0);
}
