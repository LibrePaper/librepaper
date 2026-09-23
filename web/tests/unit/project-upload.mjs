import assert from "node:assert/strict";
import { archiveProject, archiveSelection, expandArchives } from "../../src/lib/project-upload.js";
const config = {
  extensions: [".md", ".html", ".qmd", ".tex", ".typ"],
  text_extensions: [".md", ".html", ".qmd", ".tex", ".typ", ".bib", ".csv", ".json", ".yml"],
  asset_extensions: [".png", ".pdf"], derived_extensions: [".aux", ".log"],
  log_quota_bytes: 10000, storage: { per_owner: 10000 }, max_files: 200,
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
assert.throws(() => select(latex, latex.main, { ...config, max_files: 2 }), /hold 2 files/);
assert.throws(() => select(project({ "main.md": new Uint8Array([255]) })), /UTF-8/);
// A ZIP dropped into a project's explorer becomes the files inside it, with
// the wrapper folder stripped and the rendered output left behind, and the
// folders it needs named parents first. Which file is the main one is no
// longer asked: the project already has one.
{
  const archive = { file: "archive bytes", path: "paper.zip" };
  const plain = { file: "figure bytes", path: "extra.png" };
  const opened = await expandArchives([archive, plain], config, async (file, limits) => {
    assert.equal(file, "archive bytes");
    assert.equal(limits.maxBytes, config.log_quota_bytes + config.storage.per_owner);
    return entries({
      "paper/main.qmd": "# Source",
      "paper/main.html": "Generated",
      "paper/chapters/one.qmd": "# One",
      "paper/fig/plot.png": new Uint8Array([1, 2]),
      "paper/_freeze/cache.json": "cache",
    });
  });
  assert.deepEqual(opened.files.map((item) => item.path), ["main.qmd", "chapters/one.qmd", "fig/plot.png", "extra.png"]);
  assert.deepEqual(opened.folders, ["chapters", "fig"]);
  assert.equal(opened.files.at(-1).file, "figure bytes", "a file that is not an archive is passed through untouched");
  assert.equal(opened.files[0].file.name, "main.qmd", "each extracted file is named by its own basename");
}

// An archive whose candidates are ambiguous still imports: the drop is about
// the files, and the destination project already names its main file.
{
  const opened = await expandArchives([{ file: null, path: "two.zip" }], config, async () =>
    entries({ "one.md": "First", "two.md": "Second" }));
  assert.deepEqual(opened.files.map((item) => item.path), ["one.md", "two.md"]);
}

// A refusal inside the archive refuses the import whole, before anything is
// written.
{
  await assert.rejects(
    expandArchives([{ file: null, path: "bad.zip" }], config, async () => entries({ "refs.bib": "References" })),
    /no supported document/,
  );
}

console.log("project-upload: main selection, wrapper folders, exclusions, Quarto sharing, separate limits, and explorer imports passed");
