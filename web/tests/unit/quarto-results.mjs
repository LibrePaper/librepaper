import assert from "node:assert/strict";
import fs from "node:fs";
import { cellFingerprint, parseQuarto } from "../../src/lib/engines/quarto.js";
import { splitResults, tagCells } from "../../src/lib/quarto-results.js";

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

// Tagging: each executed cell gets `#| classes: lp-cell-N` directly after its
// opening fence, in the file it lives in. The input tree is left alone.
{
  const cell = (n, options = "") => `\`\`\`{r}\n${options}${n}\n\`\`\`\n`;
  const files = {
    "chap/a.qmd": `${cell("a")}\n{{< include b.qmd >}}\n`,
    "chap/b.qmd": cell("b", "#| label: b1\n"),
  };
  const main = `${cell("m1")}\n{{< include chap/a.qmd >}}\n\n${cell("m2", "#| classes: \"hidden extra\"\n#| label: m2\n")}`;
  const tree = { main: "main.qmd", texts: { ...files, "main.qmd": main } };
  const before = JSON.stringify(tree);

  const tagged = tagCells(tree);
  assert.equal(JSON.stringify(tree), before, "the input tree is not mutated");
  assert.notEqual(tagged.texts, tree.texts);
  assert.equal(tagged.main, "main.qmd");
  // Cells in document order: m1 is 0, a is 1, b (included by a) is 2, m2 is 3.
  assert.equal(tagged.texts["main.qmd"], "```{r}\n#| classes: lp-cell-0\nm1\n```\n\n{{< include chap/a.qmd >}}\n\n```{r}\n#| classes: \"hidden .extra .lp-cell-3\"\n#| label: m2\nm2\n```\n");
  assert.equal(tagged.texts["chap/a.qmd"], "```{r}\n#| classes: lp-cell-1\na\n```\n\n{{< include b.qmd >}}\n");
  assert.equal(tagged.texts["chap/b.qmd"], "```{r}\n#| classes: lp-cell-2\n#| label: b1\nb\n```\n");

  // A bare classes value takes the tag; a flow list is left alone.
  const forms = tagCells({ main: "x.qmd", texts: { "x.qmd": "```{r}\n#| classes:\n1\n```\n\n```{r}\n#| classes: [a, b]\n2\n```\n" } });
  assert.equal(forms.texts["x.qmd"], "```{r}\n#| classes: lp-cell-0\n1\n```\n\n```{r}\n#| classes: [a, b]\n2\n```\n");

  // A CRLF file gets CRLF inserted lines.
  const crlf = tagCells({ main: "c.qmd", texts: { "c.qmd": "```{r}\r\n1\r\n```\r\n" } });
  assert.equal(crlf.texts["c.qmd"], "```{r}\r\n#| classes: lp-cell-0\r\n1\r\n```\r\n");

  // The tagged source's output pairs by tag. Cell 1 (a) and cell 3 (m2) printed
  // nothing, so Quarto wrote no div for them and they get "". The tag class is
  // gone from the stored html.
  const rendered = `<main id="quarto-document-content"><div class="cell lp-cell-2"><p>b</p></div><div class="cell lp-cell-0"><p>m1</p></div></main>`;
  const split = await splitResults(rendered, main, { path: "main.qmd", files: tree.texts });
  assert.equal(split.ok, true);
  assert.deepEqual(split.record.cells.map((c) => c.html), ['<div class="cell"><p>m1</p></div>', "", '<div class="cell"><p>b</p></div>', ""]);
  assert.deepEqual(split.record.cells.map((c) => c.path ?? null), [null, "chap/a.qmd", "chap/b.qmd", null]);

  // A tagged cell whose div holds only echoed code is also "".
  const codeOnly = `<main id="quarto-document-content"><div class="cell lp-cell-0"><div class="cell-code"><pre>x</pre></div></div></main>`;
  assert.equal((await splitResults(codeOnly, "```{r}\n1\n```\n")).record.cells[0].html, "");

  // A tag outside the expected range is ignored.
  const stray = `<main id="quarto-document-content"><div class="cell lp-cell-9"><p>x</p></div></main>`;
  assert.equal((await splitResults(stray, "```{r}\n1\n```\n")).record.cells[0].html, "");

  // Without any tag the pairing stays positional, count checks included.
  assert.deepEqual(await splitResults(`<main id="quarto-document-content"><div class="cell"><p>x</p></div></main>`, "```{r}\n1\n```\n```{r}\n2\n```\n"), { ok: false, reason: "count-mismatch" });
  const untaggedOne = await splitResults(`<main id="quarto-document-content"><div class="cell"><p>x</p></div></main>`, "```{r}\n1\n```\n");
  assert.equal(untaggedOne.record.cells[0].html, '<div class="cell"><p>x</p></div>');
}

// Inline values: tagCells wraps them in the run copy, splitResults pairs them back.
{
  const { expandedInlines, inlineFingerprint } = await import("../../src/lib/engines/quarto.js");
  const mainText = [
    "---", "title: \"`r not_here`\"", "---", "",
    "Start `r a` and `{r} b`.", "",
    "| x | y |", "|---|---|", "| `r c` | z |", "",
    "```text", "`r skipped`", "```", "",
    "```{r}", "#| label: m1", "1", "```", "",
    "Tail `r d` done", "",
  ].join("\n");
  const tagged = tagCells({ main: "main.qmd", texts: { "main.qmd": mainText } });
  // Front matter and the text fence are left alone; the cell's tag goes in
  // after its fence line, and the inline edits do not move it.
  assert.equal(tagged.texts["main.qmd"], [
    "---", "title: \"`r not_here`\"", "---", "",
    "Start [`r a`]{.lp-inline-0} and [`{r} b`]{.lp-inline-1}.", "",
    "| x | y |", "|---|---|", "| [`r c`]{.lp-inline-2} | z |", "",
    "```text", "`r skipped`", "```", "",
    "```{r}", "#| classes: lp-cell-0", "#| label: m1", "1", "```", "",
    "Tail [`r d`]{.lp-inline-3} done", "",
  ].join("\n"));

  // A document without inline values is tagged exactly as before.
  assert.equal(tagCells({ main: "x.qmd", texts: { "x.qmd": "```{r}\n1\n```\n" } }).texts["x.qmd"], "```{r}\n#| classes: lp-cell-0\n1\n```\n");

  const page = [
    '<main id="quarto-document-content">',
    '<p>Start <span class="lp-inline-0">3</span> and <span class="lp-inline-1"><em onclick="x()">4</em>\n</span>.</p>',
    '<table><tr><td><span class="lp-inline-2">z</span></td></tr></table>',
    '<div class="cell lp-cell-0"><div class="cell-output"><pre>1</pre></div></div>',
    "<p>Tail 9 done</p>",
    "</main>",
  ].join("\n");
  const split = await splitResults(page, mainText, { path: "main.qmd" });
  assert.equal(split.ok, true);
  assert.equal(split.record.cells.length, 1);
  const fingerprints = await Promise.all(expandedInlines(mainText, { path: "main.qmd" }).map((record) => inlineFingerprint(record)));
  assert.equal(fingerprints.length, 4);
  // Markup inside a span is kept, its handlers and newlines are not; a missing tag gives "".
  assert.deepEqual(split.record.inlines, [
    { fingerprint: fingerprints[0], html: "3" },
    { fingerprint: fingerprints[1], html: "<em>4</em> " },
    { fingerprint: fingerprints[2], html: "z" },
    { fingerprint: fingerprints[3], html: "" },
  ]);

  // A page with no tagged span, or a document with no inline values, has no inlines key.
  const untagged = await splitResults('<main id="quarto-document-content"><div class="cell"><p>1</p></div></main>', mainText, { path: "main.qmd" });
  assert.equal(untagged.ok, true);
  assert.equal("inlines" in untagged.record, false);
  const noInline = await splitResults(page, "```{r}\n#| label: m1\n1\n```\n", { path: "main.qmd" });
  assert.equal(noInline.ok, true);
  assert.equal("inlines" in noInline.record, false);

  // An unclosed span gives no value and does not fail the split.
  const broken = await splitResults('<main id="quarto-document-content"><p>Start <span class="lp-inline-0">3</p><div class="cell lp-cell-0"><div class="cell-output"><pre>1</pre></div></div></main>', "Start `r a`\n\n```{r}\n1\n```\n", { path: "main.qmd" });
  assert.equal(broken.ok, true);
  assert.equal("inlines" in broken.record, false);

  // Inline values of an included file are tagged in that file and carry its path.
  const incMain = "Start `r a`\n\n{{< include chap/a.qmd >}}\n\n```{r}\n1\n```\n";
  const incFiles = { "chap/a.qmd": "In `r d`\n" };
  const incTagged = tagCells({ main: "main.qmd", texts: { "main.qmd": incMain, ...incFiles } });
  assert.equal(incTagged.texts["chap/a.qmd"], "In [`r d`]{.lp-inline-1}\n");
  assert.equal(incTagged.texts["main.qmd"], "Start [`r a`]{.lp-inline-0}\n\n{{< include chap/a.qmd >}}\n\n```{r}\n#| classes: lp-cell-0\n1\n```\n");
  const incPage = '<main id="quarto-document-content"><p>Start <span class="lp-inline-0">1</span></p><p>In <span class="lp-inline-1">2</span></p><div class="cell lp-cell-0"><div class="cell-output"><pre>1</pre></div></div></main>';
  const incSplit = await splitResults(incPage, incMain, { path: "main.qmd", files: incFiles });
  const [fa, fd] = await Promise.all(expandedInlines(incMain, { path: "main.qmd", files: incFiles }).map((record) => inlineFingerprint(record)));
  assert.deepEqual(incSplit.record.inlines, [
    { fingerprint: fa, html: "1" },
    { fingerprint: fd, html: "2", path: "chap/a.qmd" },
  ]);
}

// A page with no cell div still publishes its inline values; every expected cell stays, with no output.
{
  const { expandedCells, inlineFingerprint, expandedInlines } = await import("../../src/lib/engines/quarto.js");
  const source = "Value `r x`.\n\n```{r}\n#| echo: false\nx <- 1\n```\n";
  const page = '<main id="quarto-document-content"><p>Value <span class="lp-inline-0">5</span>.</p></main>';
  const split = await splitResults(page, source, { path: "main.qmd" });
  const [cell] = expandedCells(source, { path: "main.qmd" });
  assert.equal(split.ok, true);
  assert.deepEqual(split.record.cells, [{ fingerprint: await cellFingerprint(cell), label: null, html: "" }]);
  const [inline] = expandedInlines(source, { path: "main.qmd" });
  assert.deepEqual(split.record.inlines, [{ fingerprint: await inlineFingerprint(inline), html: "5" }]);
  // Without an inline value to keep, a page with no cell div is still no-cells.
  assert.deepEqual(await splitResults("<main id=\"quarto-document-content\"><p>plain</p></main>", source, { path: "main.qmd" }), { ok: false, reason: "no-cells" });
  // A backtick run longer than one is literal text: the tagged line is left as it is.
  const literal = "Shown as `` `r x` `` here.\n";
  assert.equal(tagCells({ main: "main.qmd", texts: { "main.qmd": literal } }).texts["main.qmd"], literal);
}
