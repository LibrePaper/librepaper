import assert from "node:assert/strict";
import { formatOf, previewFormatOf, sourceFormatOf } from "../../src/lib/formats.js";
import { render } from "../../src/lib/renderers.js";

// --- data files and scripts preview as their plain text -------------------------

for (const path of [
  "a.json", "b.yaml", "c.yml", "d.toml", "E.JSON", "notes.txt", "refs.bib",
  "bundle.csl", "script.r", "script.py", "script.jl", "script.lua", "mystyle.sty",
  "mystyle.cls", "mystyle.bst", "site.css", "x.csv", "X.CSV", "x.tsv", "X.TSV",
  "fig.pgf", "fig.tikz", "size10.clo", "t1x.fd", "index.ist",
]) {
  assert.equal(previewFormatOf(path), "text", `${path} previews as text`);
}

// --- document formats preview as themselves -------------------------------------

assert.equal(previewFormatOf("x.md"), "markdown", "Markdown previews as Markdown");
assert.equal(previewFormatOf("x.tex"), "latex", "LaTeX previews as LaTeX");
assert.equal(previewFormatOf("x.typ"), "typst", "Typst previews as Typst");

// --- only the empty path has no preview -----------------------------------------
// The server's allowlist, not this function, decides which files exist, so any
// other path that reaches the preview is a text file.

assert.equal(previewFormatOf(""), "", "the empty path has no preview");
assert.equal(previewFormatOf("x.dat"), "text", "an unlisted text file previews as text");

// --- the document format is unchanged for data files and packages ---------------

assert.equal(formatOf("a.json"), "", "a data file is still not a document format");
assert.equal(formatOf("mystyle.sty"), "", "a package is LaTeX but never a document");
assert.equal(formatOf("fig.pgf"), "", "a PGF figure is LaTeX but never a document");

// --- the source language covers every LaTeX source extension --------------------

for (const path of ["main.tex", "MyStyle.STY", "fig.pgf", "fig.tikz", "size10.clo", "x.xmpdata"]) {
  assert.equal(sourceFormatOf(path), "latex", `${path} is LaTeX source`);
}
assert.equal(sourceFormatOf("index.ist"), "", "makeindex styles are not TeX syntax");
assert.equal(sourceFormatOf("x.json"), "", "a data file has no source language");
assert.equal(sourceFormatOf("a.qmd"), "quarto", "Quarto is a source language");

// --- a text file renders as a highlighted page with no script ------------------

const page = await render({ main: "a.json", texts: { "a.json": "{\"k\": 1} <x>" } }, "");
assert.match(page.html, /<pre>/, "the text page has a preformatted block");
assert.match(page.html, /&lt;x&gt;/, "source is escaped");
assert.match(page.html, /<span class="tok-/, "a known language is highlighted");
assert.doesNotMatch(page.html, /<script/i, "the text page has no script");

console.log("preview format tests passed");
