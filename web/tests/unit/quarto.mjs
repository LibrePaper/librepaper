import assert from "node:assert/strict";
import fs from "node:fs";
import { QUARTO_SIDECAR, cellFingerprint, contextId, contextFingerprint, mapDraftDiagnostics, parseQuarto, quartoSidecar, translateQuartoDiagnostics } from "../../src/lib/engines/quarto.js";
import { formatOf, outputKind } from "../../src/lib/renderers.js";

const source = [
  "---", "title: Example", "execute:", "  echo: false", "---", "",
  "```text", "{{< include hidden.qmd >}}", "```", "",
  "```{r #plot}", "#| fig-cap: Plot", "plot(1)", "```", "",
  "```{r #plot}", "plot(2)", "```",
].join("\n");
const parsed = parseQuarto(source, { path: "paper.qmd" });
assert.equal(formatOf("paper.qmd"), "quarto");
assert.equal(outputKind("quarto"), "html");
assert.equal(parsed.metadata.title, "Example");
assert.equal(parsed.cells.length, 2);
assert.equal(parsed.cells[0].id, "paper.qmd#plot");
assert.equal(parsed.cells[0].ambiguous, true);
assert.equal(parseQuarto("```r\nplot(x)\n```\n", { path: "paper.qmd" }).cells.length, 0);
const unnamed = parseQuarto("```{r}\na <- 1\n```\n\n```{python}\nprint(1)\n```\n", { path: "paper.qmd" });
assert.deepEqual(unnamed.cells.map((cell) => cell.id), ["paper.qmd#cell-1", "paper.qmd#cell-2"]);
const reordered = parseQuarto("```{python}\nprint(1)\n```\n\n```{r}\n#| label: \"fig-quoted\"\nplot(1)\n```\n", { path: "paper.qmd" });
assert.deepEqual(reordered.cells.map((cell) => cell.id), ["paper.qmd#cell-1", "paper.qmd#fig-quoted"]);
const longFence = "````\n```{r}\nplot(1)\n```\n````\n";
assert.equal(parseQuarto(longFence, { path: "paper.qmd" }).cells.length, 0, "short inner fences stay inside a longer opaque fence");
const longCell = "````{r}\n#| label: long\nplot(1)\n```\n````\n";
assert.equal(parseQuarto(longCell, { path: "paper.qmd" }).cells.length, 1, "a long executable fence closes only at its own length");
const knitrComma = parseQuarto("```{r,echo=FALSE}\nhidden_one()\n```\n```{r, echo=FALSE}\nhidden_two()\n```\n```{r , echo=FALSE}\nhidden_three()\n```\n", { path: "paper.qmd" });
assert.deepEqual(knitrComma.cells.map((cell) => [cell.language, cell.options.echo]), [["r", false], ["r", false], ["r", false]]);
const paritySource = [
  "---", "title: Parity", "---", "", "A {{< include appendix.qmd >}} value.", "",
  "```r", "plot(x)", "```", "", "```{r #fig-a echo=false}", "#| fig-cap: A", "x <- 1", "plot(x)", "```", "",
  "```{python}", "print(\"hi\")", "```", "",
].join("\n");
const parity = parseQuarto(paritySource, { path: "paper.qmd" });
assert.deepEqual(parity.cells.map((cell) => cell.id), ["paper.qmd#fig-a", "paper.qmd#cell-2"]);
assert.deepEqual(parity.includes, ["{{< include appendix.qmd >}}"]);
assert.deepEqual(parity.inlineExpressions, ["A {{< include appendix.qmd >}} value."]);
assert.equal(await cellFingerprint(parity.cells[0]), "6ab5fc1e98ebfded320c52bc3d4cc6092809d66d08bff14f33a4ab6d39faea89");
assert.equal(await cellFingerprint(parity.cells[1]), "be3a76e7ab5c71d759e4831505f7e073b7cdceb84e1fe58481cd40adfa9b5b66");
// Empty format and omitted parameters match `quarto inspect`'s native
// inspection protocol; managed bundles pass their explicit format and the
// recorded parameters_sha256 through the same API.
assert.equal(await contextFingerprint(parity, { main: "paper.qmd", format: "" }), "bddb54e45fceb835195faeca99616ae92c1acb31bda668910280a62f29d5ef05");
const nativeFixture = parseQuarto(fs.readFileSync(new URL("../fixtures/quarto/r.qmd", import.meta.url), "utf8"), { path: "paper.qmd" });
assert.deepEqual(nativeFixture.cells.map((cell) => cell.id), ["paper.qmd#setup", "paper.qmd#fig-first", "paper.qmd#change-input", "paper.qmd#fig-second", "paper.qmd#tbl-summary", "paper.qmd#documented-only"]);
assert.deepEqual(await Promise.all(nativeFixture.cells.map(cellFingerprint)), [
  "d2fe7d8aea06511b6b4067f4e7b7a5976965719eb5be3fed2e87564130cc7063",
  "a7b87ee92e74d520e4b8e89949ef7a9749a5949e90f85642ee504bf10cb718a2",
  "9291218da481c0109ff45661399cf2c284772aa9dd75e6e7c382d6ce414cfa32",
  "a6fc197c4153560f065dca2cf0b1b3c99d90261d2f15a6dec2aa9563a37f1033",
  "eb4a81516a926f66462c1e02e1cac5c84cdabaf32f21c8f5ae633bb9480722fe",
  "600c7d96666f2aee6b4c25d3149512ae4fce781ddf7cc5a23bfdb985afed813c",
]);
assert.equal(await contextFingerprint(nativeFixture, { main: "paper.qmd", format: "" }), "36b939c9c7c5ab978b58d7dca0299aeea9bded225822c0851f6261f8bddac126");
const resultsSource = [
  "---", "title: Shared", "---", "",
  "```{r}", "#| label: first", "a <- 1", "```", "",
  "Between.", "",
  "```{r}", "#| label: second", "b <- 2", "```", "",
  "```{r}", "#| label: hidden", "#| include: false", "c <- 3", "```", "",
].join("\n");
const resultsParsed = parseQuarto(resultsSource, { path: "paper.qmd" });
const resultsPrints = {};
for (const cell of resultsParsed.cells) resultsPrints[cell.id] = { sha256: await cellFingerprint(cell), ambiguous: false };
const [firstPrint, secondPrint] = resultsParsed.cells.map((cell) => resultsPrints[cell.id].sha256);
const nestedDialect = [
  "::: {.callout-note #note}", "Outer", "::: {.callout-warning #inner}", "Inner", ":::", ":::", "",
  "::: {.columns}", "::: {.column}", "Column", ":::", ":::", "",
  "::: {.panel-tabset}", "## First", ":::", "",
  "::: {#fig-one}", "Figure caption", ":::", "",
  "::: {#tbl-one}", "Table caption", ":::", "",
  "See @fig-one and @tbl-one.",
].join("\n");
const nestedParsed = parseQuarto(nestedDialect, { path: "paper.qmd" });
assert.equal(nestedParsed.divs.find((div) => div.id === "inner").depth, 1);
const nestedAuthors = parseQuarto([
  "---", "authors:", "  - name: Ada Lovelace", "    affiliation: Analytical Engine", "  - name: Grace Hopper", "    affiliation: Navy", "---", "", "Text",
].join("\n"), { path: "paper.qmd" });
assert.equal(nestedAuthors.diagnostics.length, 0, "nested author metadata should not make freshness unknown");
assert.deepEqual(nestedAuthors.metadata.authors.map((author) => author.name), ["Ada Lovelace", "Grace Hopper"]);
const inlineSource = "Value: `r 1 + 1`.";
const inlineParsed = parseQuarto(inlineSource, { path: "paper.qmd" });
const inline = inlineParsed.inlineRecords[0];
const multiInline = parseQuarto("`r 1 + 1`\n`r 2 + 2`", { path: "paper.qmd" }).inlineRecords;
assert.deepEqual(multiInline.map((item) => item.id), ["paper.qmd#inline-1-0", "paper.qmd#inline-2-0"]);
const duplicateInlineSource = "Values: `r 1 + 1`, `r 1 + 1`.";
const duplicateInline = parseQuarto(duplicateInlineSource, { path: "paper.qmd" }).inlineRecords;
const incomplete = "---\ntitle: Still editing\n\n# Intro\n\nText";
assert.equal(parseQuarto(incomplete).diagnostics[0].severity, "warning");
assert.equal(await contextId({ format: "html", profiles: ["default"], parameters: { seed: 1 } }), await contextId({ format: "html", profiles: ["default"], parameters: { seed: 1 } }));
assert.notEqual(await contextId({ format: "html", parameters: { seed: 1 } }), await contextId({ format: "html", parameters: { seed: "1" } }), "parameter scalar types remain part of the selection identity");
assert.notEqual(await contextId({ format: "html" }), await contextId({ format: "pdf" }));
assert.equal((await contextFingerprint(parsed, { main: "paper.qmd" })).length, 64);
// The sidecar the Markdown module reads beside a .qmd: saved outputs placed by
// the 1-based line of their cell's opening fence.
const strings = { output_stale: "S", include_unavailable: "U {name}", include_cycle: "C {normalized}", abstract: "A" };
assert.equal(QUARTO_SIDECAR, ".librepaper/quarto.json");
const fresh = await quartoSidecar(resultsSource, {
  path: "paper.qmd", strings,
  results: { note: " Ada's run ", cells: [{ fingerprint: secondPrint, label: "second", html: "<p>TWO</p>\n" }, { fingerprint: firstPrint, label: "first", html: "<p>ONE</p>" }] },
});
assert.deepEqual(fresh.outputs, [
  { line: 5, html: "<p>ONE</p>", stale: false },
  { line: 12, html: "<p>TWO</p> ", stale: false },
]);
assert.equal(fresh.note, "Ada's run");
assert.deepEqual(fresh.strings, strings);
// Changed code, same label: stale. The include: false cell never gets output.
const stale = await quartoSidecar(resultsSource, {
  path: "paper.qmd", strings,
  results: { cells: [{ fingerprint: "1".repeat(64), label: "first", html: "<p>OLD</p>" }, { fingerprint: secondPrint, label: "second", html: "<p>TWO</p>" }, { fingerprint: "2".repeat(64), label: "hidden", html: "<p>HIDDEN</p>" }] },
});
assert.deepEqual(stale.outputs.map((o) => [o.line, o.stale]), [[5, true], [12, false]]);
// Same count, no label: positional and stale. Different count: nothing.
const positional = await quartoSidecar(resultsSource, { path: "paper.qmd", strings, results: { cells: [{ fingerprint: "3".repeat(64), label: null, html: "a" }, { fingerprint: "4".repeat(64), label: null, html: "b" }] } });
assert.deepEqual(positional.outputs.map((o) => o.stale), [true, true]);
assert.deepEqual((await quartoSidecar(resultsSource, { path: "paper.qmd", strings, results: { cells: [{ fingerprint: "3".repeat(64), label: null, html: "x" }] } })).outputs, []);
// Without saved results the file still carries the strings, with the real catalogue by default.
const bare = await quartoSidecar("Plain.\n");
assert.deepEqual(bare.outputs, []);
assert.equal(bare.note, "");
assert.equal(bare.strings.include_unavailable, "Include unavailable: {name}");
assert.equal(bare.strings.include_cycle, "Include cycle or depth limit: {normalized}");

// Diagnostics named by a catalogue key are translated from their hints.
const translated = translateQuartoDiagnostics([
  { severity: "warning", message: "quarto_include_unauthorized", hints: ["name=../secret.qmd"], file: "", line: 3, column: 1 },
  { severity: "error", message: "quarto_cell_label_duplicate", hints: ["label=fig-a"], file: "", line: 9, column: 1 },
  { severity: "error", message: "unterminated string", hints: ["h"], file: "", line: 1, column: 1 },
]);
assert.equal(translated[0].message, "Include unavailable or unauthorized: ../secret.qmd");
assert.equal(translated[0].line, 3);
assert.equal(translated[1].message, "duplicate cell label fig-a");
assert.equal(translated[2].message, "unterminated string");

// A diagnostic from the citations module is put back on qmd lines; generated lines stay unplaced.
const mapped = mapDraftDiagnostics([
  { severity: "error", message: "prose", file: "main.md", line: 3, column: 1 },
  { severity: "error", message: "generated", file: "main.md", line: 2, column: 1 },
], [1, null, 7], "main.qmd");
assert.deepEqual([mapped[0].file, mapped[0].line, mapped[0].generated], ["main.qmd", 7, false]);
assert.deepEqual([mapped[1].line, mapped[1].generated], [0, true]);
console.log("quarto: parser, sidecar, diagnostics and identity scenarios passed");
