// Quarto completion over real EditorState and CompletionContext objects. The
// cursor is the last "|" in each document (a "#|" cell option comes before it);
// it is removed before the state is built.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import schema from "../../src/lib/completion/quarto-schema.json" with { type: "json" };
import { quartoCompletion, quartoLabels } from "../../src/lib/completion/quarto.js";

function complete(doc, { format = "quarto", texts = {}, path = "main.qmd", explicit = false } = {}) {
  const pos = doc.lastIndexOf("|");
  const state = EditorState.create({ doc: doc.slice(0, pos) + doc.slice(pos + 1) });
  const context = new CompletionContext(state, pos, explicit);
  let reads = 0;
  const project = () => ({
    path,
    format,
    get texts() { reads++; return texts; },
    files: [],
  });
  const result = quartoCompletion(project)(context);
  return { result, pos, reads };
}
const labels = (result) => (result ? result.options.map((option) => option.label) : null);

// --- quartoLabels ----------------------------------------------------------
{
  const texts = {
    "a.qmd": "```{r}\n#| label: fig-plot\n```\n\nSee {#tbl-x}.\n\n```\n{#fig-hidden}\n```\n\n`{#sec-inline}`\n",
    "notes.qmd": "# Intro {#sec-intro}\n",
    "b.md": "{#fig-md}\n",
  };
  assert.deepEqual(quartoLabels(texts), [
    { id: "fig-plot", kind: "fig", path: "a.qmd", line: 2 },
    { id: "tbl-x", kind: "tbl", path: "a.qmd", line: 5 },
    { id: "sec-intro", kind: "sec", path: "notes.qmd", line: 1 },
  ]);
  assert.deepEqual(quartoLabels({}), []);
}

// --- front matter keys and values -----------------------------------------
{
  const { result, pos } = complete("---\nti|\n---\n");
  assert.equal(result.from, pos - 2);
  assert.equal(result.validFor.test("title"), true);
  const title = result.options.find((option) => option.label === "title");
  assert.equal(title.apply, "title: ");
  assert.equal(title.type, "property");
  assert.equal(title.info, "Document title");
}
{
  const { result, pos } = complete("---\nformat: ht|\n---\n");
  assert.equal(result.from, pos - 2);
  assert.ok(labels(result).includes("html"));
  assert.ok(labels(result).includes("pdf"));
}
{
  // A key without enumerated values offers nothing.
  assert.equal(complete("---\ntitle: |\n---\n").result, null);
  // Indented lines are not top-level keys.
  assert.equal(complete("---\n  ti|\n---\n").result, null);
  // Front matter only starts on the first line of the file.
  assert.equal(complete("text\n---\nti|\n---\n").result, null);
}

// --- cell options ----------------------------------------------------------
{
  const { result, pos } = complete("```{r}\n#| ec|\n```\n");
  assert.equal(result.from, pos - 2);
  const echo = result.options.find((option) => option.label === "echo");
  assert.ok(echo, "echo is a cell option");
  assert.equal(echo.apply, "echo: ");
  assert.equal(echo.type, "property");
}
{
  const { result, pos } = complete("```{python}\n#| echo: |\n```\n");
  assert.equal(result.from, pos);
  assert.deepEqual(labels(result), ["false", "fenced", "true"]);
}
{
  const { result, pos } = complete("```{r}\n#| echo: fa|\n```\n");
  assert.equal(result.from, pos - 2);
  assert.ok(labels(result).includes("false"));
}
{
  // Plain fences (no braces) are not executable cells.
  assert.equal(complete("```python\n#| ec|\n```\n").result, null);
  assert.equal(complete("```\n#| ec|\n```\n").result, null);
}

// --- cross-references ------------------------------------------------------
{
  const texts = { "chap.qmd": "```{r}\n#| label: fig-plot\n```\n\n{#tbl-x}\n" };
  const { result, pos } = complete("Intro {#sec-a}\n\nSee @fi|", { texts });
  assert.equal(result.from, pos - 2);
  assert.equal(result.validFor.test("fig-p"), true);
  assert.deepEqual(labels(result), ["sec-a", "fig-plot", "tbl-x"]);
  const plot = result.options.find((option) => option.label === "fig-plot");
  assert.equal(plot.detail, "chap.qmd:2");
}
{
  const texts = { "chap.qmd": "{#fig-plot}\n" };
  const { result, pos } = complete("See @fig-|", { texts });
  assert.equal(result.from, pos - 4);
  assert.deepEqual(labels(result), ["fig-plot"]);
}
{
  // A bare "@" waits for a typed prefix unless completion is explicit.
  const texts = { "chap.qmd": "{#fig-plot}\n" };
  assert.equal(complete("See @|", { texts }).result, null);
  assert.deepEqual(labels(complete("See @|", { texts, explicit: true }).result), ["fig-plot"]);
}
{
  // Email addresses and words that merely contain "@" do not trigger.
  assert.equal(complete("mail me at jo@fi|", { texts: { "a.qmd": "{#fig-a}\n" } }).result, null);
}
{
  // Nothing fires inside code: fenced code, inline code, or cell option values.
  const texts = { "chap.qmd": "{#fig-plot}\n" };
  assert.equal(complete("```{r}\n#| fig-cap: \"@fi|\"\n```\n", { texts }).result, null);
  assert.equal(complete("```\nSee @fi|\n```\n", { texts }).result, null);
  assert.equal(complete("Use `@fi|", { texts }).result, null);
}
{
  // Other project files are read only when the trigger matches.
  const { result, reads } = complete("Hello world|", { texts: { "a.qmd": "{#fig-a}\n" } });
  assert.equal(result, null);
  assert.equal(reads, 0);
}

// --- other formats ---------------------------------------------------------
{
  const texts = { "chap.qmd": "{#fig-plot}\n" };
  assert.equal(complete("---\nti|\n---\n", { format: "markdown" }).result, null);
  assert.equal(complete("See @fi|", { format: "markdown", texts }).result, null);
  assert.equal(complete("#| ec|", { format: "latex" }).result, null);
}

// --- quarto-schema.json ----------------------------------------------------
{
  const documentNames = schema.document.map((entry) => entry.name);
  const cellNames = schema.cell.map((entry) => entry.name);
  assert.ok(documentNames.includes("title"));
  assert.ok(documentNames.includes("format"));
  assert.ok(cellNames.includes("fig-cap"));
  assert.ok(cellNames.includes("echo"));
  for (const entry of [...schema.document, ...schema.cell]) {
    assert.equal("hidden" in entry, false, `${entry.name} is hidden`);
    assert.ok(entry.values.every((value) => typeof value === "string"));
  }
  assert.ok(JSON.stringify(schema).length < 200_000);
}
