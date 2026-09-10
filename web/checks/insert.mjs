import assert from "node:assert/strict";
import { INSERT_ACTIONS, insertionAvailability, buildInsertion, gatherInsertTargets } from "../src/lib/insert.js";
assert.ok(INSERT_ACTIONS.length >= 30);
for (const format of ["latex", "typst", "markdown", "quarto"]) {
  const c = { format, text: "# Existing {#existing}\n", selection: { from: 0, to: 0, text: "" }, bibliography: [{ key: "smith2020", title: "A paper" }] };
  assert.equal(insertionAvailability("heading", c).enabled, true);
  assert.ok(buildInsertion("heading", { title: "Methods", level: 2 }, c).text.length);
  assert.ok(buildInsertion("table", { rows: 2, columns: 2 }, c).text.includes(format === "latex" ? "tabular" : format === "typst" ? "#table" : "|"));
  assert.ok(buildInsertion("citation", { keys: ["smith2020"] }, c).text.includes(format === "latex" ? "cite" : "@smith2020"));
}
assert.equal(insertionAvailability("abstract", { format: "markdown" }).enabled, false);
assert.match(buildInsertion("display-math", {}, { format: "latex" }).text, /equation/);
assert.match(buildInsertion("display-math", {}, { format: "typst" }).text, /\$/);
assert.match(buildInsertion("numbered-list", { }, { format: "markdown", selection: { text: "one\ntwo" } }).text, /1\. one/);
assert.deepEqual(gatherInsertTargets({ format: "markdown", text: "# Intro {#intro}" })[0].id, "intro");
console.log("insert: registry, capabilities, generators, targets passed");
