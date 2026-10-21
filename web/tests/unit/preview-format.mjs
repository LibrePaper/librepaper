import assert from "node:assert/strict";
import { formatOf, previewFormatOf } from "../../src/lib/formats.js";
import { render } from "../../src/lib/renderers.js";

// --- data files and scripts preview as their plain text -------------------------

for (const path of [
  "a.json", "b.yaml", "c.yml", "d.toml", "E.JSON", "notes.txt", "refs.bib",
  "bundle.csl", "script.r", "script.py", "script.jl", "script.lua", "mystyle.sty",
  "mystyle.cls", "mystyle.bst", "site.css", "x.csv", "X.CSV", "x.tsv", "X.TSV",
]) {
  assert.equal(previewFormatOf(path), "text", `${path} previews as text`);
}

// --- document formats preview as themselves -------------------------------------

assert.equal(previewFormatOf("x.md"), "markdown", "Markdown previews as Markdown");
assert.equal(previewFormatOf("x.tex"), "latex", "LaTeX previews as LaTeX");
assert.equal(previewFormatOf("x.typ"), "typst", "Typst previews as Typst");

// --- everything else has no preview ---------------------------------------------

for (const path of ["x.dat", ""]) {
  assert.equal(previewFormatOf(path), "", `${path} has no preview`);
}

// --- the document format is unchanged for data files ----------------------------

assert.equal(formatOf("a.json"), "", "a data file is still not a document format");

// --- a text file renders as a highlighted page with no script ------------------

const page = await render({ main: "a.json", texts: { "a.json": "{\"k\": 1} <x>" } }, "");
assert.match(page.html, /<pre>/, "the text page has a preformatted block");
assert.match(page.html, /&lt;x&gt;/, "source is escaped");
assert.match(page.html, /<span class="tok-/, "a known language is highlighted");
assert.doesNotMatch(page.html, /<script/i, "the text page has no script");

console.log("preview format tests passed");
