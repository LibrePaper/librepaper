import assert from "node:assert/strict";
import { archiveProject, archiveSelection } from "../../src/lib/project-upload.js";
const config = {
  extensions: [".md", ".html", ".qmd", ".tex", ".typ"],
  text_extensions: [".md", ".html", ".qmd", ".tex", ".typ", ".bib", ".csv", ".json", ".yml"],
  asset_extensions: [".png", ".pdf"], derived_extensions: [".aux", ".log"],
  max_document: 10000, max_assets: 10000, max_asset: 5000, max_files: 200,
};
const entries = (files) => Object.entries(files).map(([path, value]) => ({ path, bytes: typeof value === "string" ? new TextEncoder().encode(value) : value }));
const project = (files) => archiveProject(entries(files), config);
const select = (value, main = value.main, limits = config) => archiveSelection(value, main, limits);

const latex = project({ "paper/main.tex": "Title", "paper/refs.bib": "References", "paper/fig.png": new Uint8Array([1,2]), "paper/main.pdf": "generated", "paper/main.aux": "aux", "paper/.git/config": "private", "__MACOSX/._main.tex": "metadata" });
assert.equal(latex.main, "main.tex");
assert.deepEqual(select(latex).files.map((file) => file.path), ["main.tex", "refs.bib", "fig.png"]);
assert.ok(select(latex).skipped.includes("main.pdf"));
assert.deepEqual(latex.candidates, ["main.tex"]);
const ambiguous = project({ "one.md": "First", "two.md": "Second", "refs.bib": "References" });
assert.equal(ambiguous.main, "");
assert.deepEqual(ambiguous.candidates, ["one.md", "two.md"]);
assert.throws(() => select(ambiguous), /Choose the document/);
assert.equal(select(ambiguous, "two.md").files.length, 3);
const nested = project({ "chapters/paper.tex": "Text", "fig.png": "Image" });
assert.equal(nested.main, "");
assert.deepEqual(nested.candidates, ["chapters/paper.tex"]);
assert.equal(select(nested, "chapters/paper.tex").files.length, 2);
assert.throws(() => project({ "refs.bib": "References" }), /no supported document/);
assert.throws(() => project({ "main.md": "a", "MAIN.md": "b" }), /share one name/);

const quarto = project({ "main.qmd": "# Source", "main.html": "Generated", "main.md": "Generated", "refs.bib": "References", "data.csv": "private", "_freeze/result.json": "cache", "_site/index.html": "generated", "figure.png": new Uint8Array([3,4]) });
assert.equal(quarto.main, "main.qmd");
assert.deepEqual(select(quarto).files.map((file) => file.path), ["main.qmd", "refs.bib", "figure.png"]);
assert.ok(select(quarto, "main.html").files.some((file) => file.path === "main.html"), "changing main recomputes exclusions");
const explicit = project({ "main.qmd": "# Source", "data.csv": "shared", ".librepaper-share.json": '{"include":["data.csv"]}' });
assert.ok(select(explicit).files.some((file) => file.path === "data.csv"));
assert.throws(() => select(project({ "main.qmd": "# Source", ".librepaper-share.json": '{"include":["missing.csv"]}' })), /missing/);
assert.throws(() => project({ "main.qmd": "# Source", ".librepaper-share.json": '{"include":"data.csv"}' }), /include list/);
assert.throws(() => select(ambiguous, "one.md", { ...config, max_document: 2 }), /document size limit/);
assert.throws(() => select(latex, latex.main, { ...config, max_assets: 1 }), /combined asset/);
assert.throws(() => select(latex, latex.main, { ...config, max_asset: 1 }), /figure exceeds/);
assert.throws(() => select(latex, latex.main, { ...config, max_files: 2 }), /hold 2 files/);
assert.throws(() => select(project({ "main.md": new Uint8Array([255]) })), /UTF-8/);
console.log("project-upload: main selection, wrapper folders, exclusions, Quarto sharing, and separate limits passed");
