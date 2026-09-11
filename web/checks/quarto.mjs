import assert from "node:assert/strict";
import fs from "node:fs";
import { cellFingerprint, composeDraft, contextId, contextFingerprint, mapQuartoDiagnostics, outputMarkup, parseQuarto, safeFragment, virtualTree } from "../src/lib/engines/quarto.js";
import { formatOf, outputKind } from "../src/lib/renderers.js";
import { quartoRequest } from "../src/lib/latex/local.js";

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
// The draft never guesses at echo/include/eval semantics: with no saved
// output it shows the whole cell verbatim, fence lines included, wrapped in
// an outer fence longer than any fence run already inside the cell text.
const echoFalseDraft = composeDraft("```{r,echo=FALSE}\nhidden_one()\n```\n").markdown;
assert.match(echoFalseDraft, /hidden_one/);
assert.match(echoFalseDraft, /```\{r,echo=FALSE\}/);
assert.match(echoFalseDraft, /^~~~\n/);
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
const nativeFixture = parseQuarto(fs.readFileSync(new URL("../../crates/librepaper/src/tests/fixtures/quarto/r.qmd", import.meta.url), "utf8"), { path: "paper.qmd" });
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
assert.match(composeDraft(source, { expandIncludes: { "hidden.qmd": "DO NOT INSERT" } }).markdown, /\{\{< include hidden\.qmd >\}\}/);
const includeDraft = composeDraft("Before {{< include outer.qmd >}} after.", {
  expandIncludes: {
    "outer.qmd": "Outer {{< include inner.qmd >}}\n\n```{r}\n{{< include secret.qmd >}}\n```",
    "inner.qmd": "Inner text",
    "secret.qmd": "SECRET",
  },
});
assert.match(includeDraft.markdown, /Outer Inner text/);
assert.match(includeDraft.markdown, /\{\{< include secret\.qmd >\}\}/, "include directives inside included code stay opaque");
const cycleDraft = composeDraft("{{< include a.qmd >}}", {
  expandIncludes: { "a.qmd": "A {{< include b.qmd >}}", "b.qmd": "B {{< include a.qmd >}}" },
  maxIncludeDepth: 4,
});
assert.match(cycleDraft.markdown, /Include cycle or depth limit/);
assert.ok(cycleDraft.diagnostics.some((diagnostic) => diagnostic.generated && diagnostic.line === 0));
// Both #plot cells are ambiguous (duplicate label), so neither maps to the
// saved output; the draft falls back to showing each cell verbatim.
assert.match(composeDraft(source, { bundle: { cells: [{ id: "paper.qmd#plot", coverage: "captured", outputs: [{ kind: "text", text: "result" }] }] } }).markdown, /```\{r #plot\}\nplot\(2\)\n```/);
const dialect = [
  "---", "title: Draft title", "authors:", "  - name: Ada Lovelace", "date: 1843", "abstract: A short abstract", "---", "",
  "::: {.callout-note #intro}", "See @fig-trend.", ":::", "",
  "```{r #fig-trend}", "#| fig-cap: Trend", "plot(1)", "```",
].join("\n");
const dialectDraft = composeDraft(dialect, { bundle: { cells: [{ id: "main.qmd#fig-trend", outputs: [{ kind: "image", url: "/trend.png" }] }] } }).markdown;
assert.match(dialectDraft, /quarto-title-block/);
// Fenced divs are never interpreted: the `:::` lines carry through verbatim,
// with no callout class or wrapper markup.
assert.match(dialectDraft, /^::: \{\.callout-note #intro\}$/m);
assert.match(dialectDraft, /^:::$/m);
assert.doesNotMatch(dialectDraft, /quarto-callout/);
assert.match(dialectDraft, /href="#fig-trend"/);
assert.match(dialectDraft, /id="fig-trend"/);
assert.doesNotMatch(dialectDraft, /\n---\n/);
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
const nestedDraft = composeDraft(nestedDialect, { path: "paper.qmd" }).markdown;
// All fenced div lines carry through verbatim, including nested ones; no
// callout/columns/tabset class or wrapper markup is generated.
for (const fenceLine of [
  "::: {.callout-note #note}", "::: {.callout-warning #inner}",
  "::: {.columns}", "::: {.column}", "::: {.panel-tabset}",
  "::: {#fig-one}", "::: {#tbl-one}",
]) assert.match(nestedDraft, new RegExp(`^${fenceLine.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`, "m"));
assert.doesNotMatch(nestedDraft, /quarto-callout|quarto-columns|quarto-column\b|quarto-tabset|data-quarto-tabset/);
// Crossref resolution still works: it is independent of div rendering and
// reads figure/table numbering straight from the parsed div labels.
assert.match(nestedDraft, /href="#fig-one">Figure 1/);
assert.match(nestedDraft, /href="#tbl-one">Table 1/);
const compactCallout = composeDraft(":::{.callout-note}\nCompact note.\n:::\n").markdown;
assert.equal(compactCallout, ":::{.callout-note}\nCompact note.\n:::\n", "a compact div fence carries through verbatim with no interpretation");
const nestedAuthors = parseQuarto([
  "---", "authors:", "  - name: Ada Lovelace", "    affiliation: Analytical Engine", "  - name: Grace Hopper", "    affiliation: Navy", "---", "", "Text",
].join("\n"), { path: "paper.qmd" });
assert.equal(nestedAuthors.diagnostics.length, 0, "nested author metadata should not make freshness unknown");
assert.deepEqual(nestedAuthors.metadata.authors.map((author) => author.name), ["Ada Lovelace", "Grace Hopper"]);
const unauthorizedInclude = composeDraft("{{< include ../secret.qmd >}}", {
  expandIncludes: { "../secret.qmd": "must never appear", "safe.qmd": "safe" },
});
assert.doesNotMatch(unauthorizedInclude.markdown, /must never appear/);
assert.match(unauthorizedInclude.markdown, /Include unavailable/);
const inlineSource = "Value: `r 1 + 1`.";
const inlineParsed = parseQuarto(inlineSource, { path: "paper.qmd" });
const inline = inlineParsed.inlineRecords[0];
const multiInline = parseQuarto("`r 1 + 1`\n`r 2 + 2`", { path: "paper.qmd" }).inlineRecords;
assert.deepEqual(multiInline.map((item) => item.id), ["paper.qmd#inline-1-0", "paper.qmd#inline-2-0"]);
const duplicateInlineSource = "Values: `r 1 + 1`, `r 1 + 1`.";
const duplicateInline = parseQuarto(duplicateInlineSource, { path: "paper.qmd" }).inlineRecords;
const duplicateDraft = composeDraft(duplicateInlineSource, {
  path: "paper.qmd", currentContext: "context-1",
  inlineValues: duplicateInline.map((item) => ({ ...item, context_sha256: "context-1", value: "2" })),
});
assert.equal((duplicateDraft.markdown.match(/quarto-inline-value/g) || []).length, 2);
const capturedInline = composeDraft(inlineSource, {
  path: "paper.qmd", currentContext: "context-1",
  inlineValues: { [inline.id]: { id: inline.id, expression: inline.expression, line: inline.line, context_sha256: "context-1", value: "2" } },
});
assert.match(capturedInline.markdown, /quarto-inline-value[^>]*>2</);
const staleInline = composeDraft(inlineSource, {
  path: "paper.qmd", currentContext: "context-2",
  inlineValues: { [inline.id]: { id: inline.id, expression: inline.expression, line: inline.line, context_sha256: "context-1", value: "2" } },
});
assert.match(staleInline.markdown, /`r 1 \+ 1`/);
const mappedTree = await virtualTree({ main: "main.qmd", texts: { "main.qmd": "---\ntitle: Map\n---\n\nProse line\n\n```{r}\nplot(1)\n```\n" } });
const mappedDiagnostics = mapQuartoDiagnostics([
  { severity: "error", message: "prose", file: "main.md", line: 3, column: 1 },
  { severity: "error", message: "generated", file: "main.md", line: 1, column: 1 },
], mappedTree);
assert.equal(mappedDiagnostics[0].file, "main.qmd");
assert.ok(mappedDiagnostics[0].line > 0);
assert.equal(mappedDiagnostics[1].line, 0);
assert.equal(mappedDiagnostics[1].generated, true);
const incomplete = "---\ntitle: Still editing\n\n# Intro\n\nText";
assert.match(composeDraft(incomplete).markdown, /Still editing/);
assert.equal(parseQuarto(incomplete).diagnostics[0].severity, "warning");
// Node has no inert HTML parser: snippets must remain escaped literal text.
assert.doesNotMatch(safeFragment('<img src="x" onerror=alert(1)>'), /<img\b/);
assert.doesNotMatch(safeFragment('<a href="javascript:alert(1)">x</a>'), /<a\b/);
assert.doesNotMatch(safeFragment('<img src="//tracker.invalid/x.png">'), /<img\b/);
assert.equal(await contextId({ format: "html", profiles: ["default"], parameters: { seed: 1 } }), await contextId({ format: "html", profiles: ["default"], parameters: { seed: 1 } }));
assert.notEqual(await contextId({ format: "html", parameters: { seed: 1 } }), await contextId({ format: "html", parameters: { seed: "1" } }), "parameter scalar types remain part of the selection identity");
assert.notEqual(await contextId({ format: "html" }), await contextId({ format: "pdf" }));
assert.equal((await contextFingerprint(parsed, { main: "paper.qmd" })).length, 64);
const request = quartoRequest({ job: { binding: "binding-1", id: "job-1" }, entrypoint: "paper.qmd", files: [{ path: "paper.qmd", sha256: "a".repeat(64), size: 1 }] });
assert.equal(request.kind, "quarto");
assert.equal(request.quarto.binding_id, "binding-1");
assert.throws(() => quartoRequest({ job: { binding: "x" }, entrypoint: "../paper.qmd" }));

const original = "```{r}\nplot(x)\n```\n";
const originalCell = parseQuarto(original).cells[0];
const saved = { cells: [{ id:originalCell.id, source_path:"main.qmd", source_sha256:await cellFingerprint(originalCell), coverage:"captured", outputs:[{ kind:"text", text:"original result" }] }] };
const inserted = "```{r}\nother()\n```\n" + original;
const movedDraft = await virtualTree({ main:"main.qmd", texts:{"main.qmd":inserted,"main.md":"Companion source"} }, { bundle:saved });
assert.equal(movedDraft.texts["main.md"], "Companion source");
assert.match(movedDraft.quarto.markdown, /original result/);
// The inserted cell has no matching saved output, so it renders verbatim
// (fence lines included) ahead of the matched cell's saved output.
assert.match(movedDraft.quarto.markdown, /```\{r\}\nother\(\)\n```/);
assert.ok(movedDraft.quarto.markdown.indexOf("other()") < movedDraft.quarto.markdown.indexOf("original result"));
const duplicatedDraft = await virtualTree({ main:"main.qmd", texts:{"main.qmd":original+original} }, { bundle:saved });
assert.doesNotMatch(duplicatedDraft.quarto.markdown, /original result/);
const multiOutput = outputMarkup({ outputs: [
  { kind:"image", url:"/a.png", caption:"Panel A" },
  { kind:"image", url:"/b.png", caption:"Panel B" },
] }, {}, { label:"fig-panels", options:{ "fig-cap":"Combined caption" } });
assert.equal((multiOutput.match(/id="fig-panels"/g) || []).length, 1);
assert.match(multiOutput, /Panel A/);
assert.match(multiOutput, /Panel B/);
assert.doesNotMatch(multiOutput, /Combined caption/);
const editedCaption = outputMarkup({ outputs:[{ kind:"table", html:"<table></table>", caption:"Old caption" }] }, {}, { label:"tbl-summary", options:{ "tbl-cap":"Current caption" } });
assert.match(editedCaption, /Current caption/);
assert.doesNotMatch(editedCaption, /Old caption/);
console.log("quarto: parser, draft, identity, protocol, and sanitizer scenarios passed");
assert.doesNotMatch(composeDraft("{{< include hidden.qmd >}}", {expandIncludes:{"hidden.qmd":"```{r}\n#| include: false\nsecret_hidden_code()\n```\nVisible prose"}}).markdown, /secret_hidden_code/);
