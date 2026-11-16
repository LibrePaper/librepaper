import assert from "node:assert/strict";
import { staleCells } from "../../src/lib/quarto-results.js";
import { cellFingerprint, executedCells, parseQuarto } from "../../src/lib/engines/quarto.js";

const qmd = "# T\n\n```{r}\n1 + 1\n```\n\n```{r}\n2 + 2\n```\n";
const cells = await Promise.all(executedCells(parseQuarto(qmd, { path: "a.qmd" })).map(async (cell) => ({
  fingerprint: await cellFingerprint(cell), label: null, html: "<div class=\"cell\"></div>",
})));
assert.equal(cells.length, 2);

// Nothing changed since the run: nothing is stale.
assert.equal(await staleCells(cells, qmd, { path: "a.qmd" }), 0);
// One cell's code changed.
assert.equal(await staleCells(cells, qmd.replace("2 + 2", "3 + 3"), { path: "a.qmd" }), 1);
// A cell was added after the run.
assert.equal(await staleCells(cells, `${qmd}\n\`\`\`{r}\n9\n\`\`\`\n`, { path: "a.qmd" }), 1);
// No saved output at all: every executed cell is stale.
assert.equal(await staleCells([], qmd, { path: "a.qmd" }), 2);
// A document without code has nothing to be stale.
assert.equal(await staleCells(cells, "# Only prose\n", { path: "a.qmd" }), 0);
// A cell switched off with include: false is not executed, so is never counted.
assert.equal(await staleCells([], "```{r}\n#| include: false\n1\n```\n", { path: "a.qmd" }), 0);
// A cell duplicated after the run: one saved output covers only one of the two.
const once = "```{r}\n1 + 1\n```\n";
const [onlyCell] = await Promise.all(executedCells(parseQuarto(once, { path: "a.qmd" })).map(async (cell) => ({
  fingerprint: await cellFingerprint(cell), label: null, html: "<div class=\"cell\"></div>",
})));
assert.equal(await staleCells([onlyCell], `${once}\n${once}`, { path: "a.qmd" }), 1);
