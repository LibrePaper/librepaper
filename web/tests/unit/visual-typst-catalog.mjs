import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { TYPST_FUNCTIONS } from "../../src/lib/visual-markup/languages/typst/catalog.ts";
import { TYPST_SYMBOLS } from "../../src/lib/visual-markup/languages/typst/symbols.ts";

// Every function and symbol named in the Typst target list must be known to the
// catalog. The list is the fixture: its table rows start with a priority, and the
// second cell (the syntax column) holds the calls.

const here = dirname(fileURLToPath(import.meta.url));
const fixture = readFileSync(join(here, "../fixtures/typst-commands.md"), "utf8");
const rows = fixture.split("\n").filter((line) => /^\| P[0-3] \|/.test(line));

// Heads that are not functions the catalog covers: statements (the lexer handles
// them), the sym and emoji namespaces (their own tables), argument sink methods,
// and the placeholders the fixture writes where a name goes.
const NOT_FUNCTIONS = new Set([
  "set", "show", "let", "import", "include", "context", "if", "for", "while",
  "return", "break", "continue", "sym", "emoji", "name", "element", "template", "args",
]);

// `#name` forms: the dotted name after the hash. A `*` segment marks a family.
const HASH_NAME = /#([a-z][\w*-]*(?:\.[\w*-]+)*)/g;
// Bare `name(` forms. A dot or hash before the name makes it a method or a call
// written with a hash, and a name after set, show, let, import or include is a
// statement target, not a call.
const BARE_CALL = /(?<![\w#.-])(?<!\b(?:set|show|let|import|include)\s+)([a-z][\w-]*(?:\.[a-z][\w-]*)*)\(/g;

const functionNames = new Set();
const symbolPaths = new Set();
const symbolFamilies = new Set();
for (const row of rows) {
  const syntax = row.split("|")[2] ?? "";
  for (const match of syntax.matchAll(HASH_NAME)) {
    const segments = match[1].split(".");
    const [head] = segments;
    if (head === "sym") {
      const path = segments.slice(1).join(".");
      if (path === "*" || path === "name") continue;
      if (path.endsWith(".*")) symbolFamilies.add(path.slice(0, -1));
      else symbolPaths.add(path);
      continue;
    }
    if (head === "emoji" || NOT_FUNCTIONS.has(head)) continue;
    functionNames.add(segments.filter((segment) => segment !== "*").join("."));
  }
  for (const match of syntax.matchAll(BARE_CALL)) {
    const [head] = match[1].split(".");
    if (NOT_FUNCTIONS.has(head)) continue;
    functionNames.add(match[1]);
  }
}

// The parse must have found the list; an empty parse would pass vacuously. The
// symbol floor is the nine concrete names the fixture writes (bullet, ellipsis,
// copyright, registered, trademark, degree, section, paragraph, checkmark).
assert.ok(rows.length > 200, `only ${rows.length} table rows were parsed from the fixture`);
assert.ok(functionNames.size > 100, `only ${functionNames.size} functions were parsed from the fixture`);
assert.ok(symbolPaths.size >= 9, `only ${symbolPaths.size} concrete symbols were parsed from the fixture`);

// A dotted name is covered by its full name or by its first segment, so that
// `table.cell` is covered by `table` and `calc.min` by `calc`.
const entryFor = (name) => TYPST_FUNCTIONS.get(name) ?? TYPST_FUNCTIONS.get(name.split(".")[0]);
const symbolKeys = [...TYPST_SYMBOLS.keys()];

const problems = [];
const missingFunctions = [...functionNames].filter((name) => !entryFor(name)).sort();
const missingSymbols = [...symbolPaths].filter((path) => !TYPST_SYMBOLS.has(path)).sort();
const missingFamilies = [...symbolFamilies]
  .filter((prefix) => !symbolKeys.some((key) => key.startsWith(prefix)))
  .sort();
if (missingFunctions.length) problems.push(`functions missing from TYPST_FUNCTIONS (${missingFunctions.length}): ${missingFunctions.map((name) => `#${name}`).join(", ")}`);
if (missingSymbols.length) problems.push(`symbols missing from TYPST_SYMBOLS (${missingSymbols.length}): ${missingSymbols.map((path) => `#sym.${path}`).join(", ")}`);
if (missingFamilies.length) problems.push(`symbol families with no entry in TYPST_SYMBOLS: ${missingFamilies.map((prefix) => `#sym.${prefix}*`).join(", ")}`);

// Every entry renders through the same Render type as the LaTeX catalog.
const renderKinds = ["mark", "declaration", "hide", "symbol", "chip", "widget", "source"];
for (const [name, spec] of TYPST_FUNCTIONS) {
  if (!renderKinds.includes(spec.render?.kind)) problems.push(`#${name} has render kind "${spec.render?.kind}"`);
}

// One failure that lists everything missing, so a single run shows the whole gap.
assert.equal(problems.length, 0, `the Typst catalog does not cover the target list:\n- ${problems.join("\n- ")}`);
