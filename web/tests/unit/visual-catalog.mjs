import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { COMMANDS, ENVIRONMENTS } from "../../src/lib/visual-markup/languages/latex/catalog.ts";

// Every command and environment named in the target list must be known to the
// catalog. The list is the fixture: its table rows start with a priority.

const here = dirname(fileURLToPath(import.meta.url));
const fixture = readFileSync(join(here, "../fixtures/latex-commands.md"), "utf8");
const rows = fixture.split("\n").filter((line) => /^\| P[0-3] \|/.test(line));

// Backticked text in one cell. Two escapes are masked first: a doubled
// backslash (`\\`) and a backslash before a backtick (`\``), so that the
// backtick pairing stays aligned.
const tokens = (cell) => Array.from(
  cell.replace(/\\\\/g, "\u0002").replace(/\\`/g, "\u0001").matchAll(/`([^`]+)`/g),
  (match) => match[1],
);

// Splits a table row into its cells. Pipes inside backticks (`\verb|...|`)
// are not separators.
const cellsOf = (row) => row
  .replace(/\\\\/g, "\u0002")
  .replace(/\\`/g, "\u0001")
  .replace(/`[^`]*`/g, (match) => match.replaceAll("|", "\u0003"))
  .split("|")
  .map((cell) => cell.replaceAll("\u0003", "|").trim());

const commandNames = new Set();
const environmentNames = new Set();
const beginNames = new Set();
for (const row of rows) {
  const cells = cellsOf(row);
  // cells[0] is empty, cells[1] the priority, cells[2] the commands, cells[3] the notes.
  for (const cell of cells.slice(2, 4)) {
    for (const token of tokens(cell)) {
      // \begin{name} forms. The syntax table writes `\begin{name} ... \end{name}`
      // with `name` as a placeholder, not an environment.
      for (const match of token.matchAll(/\\begin\{([A-Za-z]+)\*?\}/g)) {
        if (match[1] !== "name") beginNames.add(match[1]);
      }
      // Plain \name tokens: no argument syntax, no symbols.
      if (/[<:|[{]/.test(token)) continue;
      const command = token.match(/^\\([A-Za-z@]+)\*?$/);
      if (command) commandNames.add(command[1]);
    }
  }
  // Plain environment names only come from the commands column.
  for (const token of tokens(cells[2] ?? "")) {
    const plain = token.match(/^([a-z]+)\*?$/);
    if (plain) environmentNames.add(plain[1]);
  }
}

// The parse must have found the list; an empty parse would pass vacuously.
assert.ok(rows.length > 200, `only ${rows.length} table rows were parsed from the fixture`);
assert.ok(commandNames.size > 100, `only ${commandNames.size} commands were parsed from the fixture`);
assert.ok(environmentNames.size > 10, `only ${environmentNames.size} environments were parsed from the fixture`);

const problems = [];
const missingCommands = [...commandNames].filter((name) => !COMMANDS.has(name)).sort();
const missingEnvironments = [...new Set([...environmentNames, ...beginNames])]
  .filter((name) => !ENVIRONMENTS.has(name))
  .sort();
if (missingCommands.length) problems.push(`commands missing from COMMANDS (${missingCommands.length}): ${missingCommands.map((name) => `\\${name}`).join(", ")}`);
if (missingEnvironments.length) problems.push(`environments missing from ENVIRONMENTS (${missingEnvironments.length}): ${missingEnvironments.join(", ")}`);

// Every spec has an argument string of x, s, o, m letters and a known render kind.
const commandKinds = ["mark", "declaration", "hide", "symbol", "chip", "widget", "source"];
const environmentKinds = ["theorem", "proof", "abstract", "keywords", "quote", "align", "code", "opaque", "bibliography", "columns", "box", "plain"];
for (const [name, spec] of COMMANDS) {
  if (!/^[som]*$/.test(spec.args)) problems.push(`\\${name} has args "${spec.args}"`);
  if (!commandKinds.includes(spec.render?.kind)) problems.push(`\\${name} has render kind "${spec.render?.kind}"`);
}
for (const [name, spec] of ENVIRONMENTS) {
  if (!/^[som]*$/.test(spec.args)) problems.push(`environment ${name} has args "${spec.args}"`);
  if (!environmentKinds.includes(spec.render?.kind)) problems.push(`environment ${name} has render kind "${spec.render?.kind}"`);
}

// One failure that lists everything missing, so a single run shows the whole gap.
assert.equal(problems.length, 0, `the LaTeX catalog does not cover the target list:\n- ${problems.join("\n- ")}`);
