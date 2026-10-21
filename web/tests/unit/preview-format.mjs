import assert from "node:assert/strict";
import { formatOf, previewFormatOf } from "../../src/lib/formats.js";

// --- data files preview as their plain text -------------------------------------

for (const path of ["a.json", "b.yaml", "c.yml", "d.toml", "E.JSON"]) {
  assert.equal(previewFormatOf(path), "text", `${path} previews as text`);
}

// --- document formats preview as themselves -------------------------------------

assert.equal(previewFormatOf("x.md"), "markdown", "Markdown previews as Markdown");
assert.equal(previewFormatOf("x.tex"), "latex", "LaTeX previews as LaTeX");
assert.equal(previewFormatOf("x.typ"), "typst", "Typst previews as Typst");

// --- everything else has no preview ---------------------------------------------

for (const path of ["x.csv", "x.bib", "x.txt", ""]) {
  assert.equal(previewFormatOf(path), "", `${path} has no preview`);
}

// --- the document format is unchanged for data files ----------------------------

assert.equal(formatOf("a.json"), "", "a data file is still not a document format");

console.log("preview format tests passed");
