// Path helpers behind completion: directory names, relative labels and file options.
import assert from "node:assert/strict";
import { dirname, relativePath, pathOptions } from "../../src/lib/completion/paths.js";

// --- dirname ----------------------------------------------------------------
assert.equal(dirname("a/b/c.tex"), "a/b");
assert.equal(dirname("c.tex"), "");
assert.equal(dirname("./a//b/../c.tex"), "a");

// --- relativePath -----------------------------------------------------------
// Sibling of the base directory.
assert.equal(relativePath("chapters", "chapters/b.tex"), "b.tex");
// Parent directory.
assert.equal(relativePath("chapters", "main.tex"), "../main.tex");
// From the root.
assert.equal(relativePath("", "a/b/c.tex"), "a/b/c.tex");
// Nested: shared prefix, then up, then down.
assert.equal(relativePath("a/b", "a/c/d.tex"), "../c/d.tex");
assert.equal(relativePath("a/b/c", "a/x/y.tex"), "../../x/y.tex");
// Target inside the base directory.
assert.equal(relativePath("a/b", "a/b/c.tex"), "c.tex");
assert.equal(relativePath("a", "a/b/c.tex"), "b/c.tex");

// --- pathOptions ------------------------------------------------------------
const files = ["main.tex", "chapters/intro.tex", "chapters/fig.png", "figs/x.PDF", "notes.md"];
const labels = (options) => options.map((option) => option.label);

// Extension filter, case-insensitive on the file's extension.
assert.deepEqual(labels(pathOptions(files, "chapters", { extensions: ["tex"] })), ["../main.tex", "intro.tex"]);
assert.deepEqual(labels(pathOptions(files, "chapters", { extensions: ["pdf"] })), ["../figs/x.PDF"]);
assert.deepEqual(labels(pathOptions(files, "chapters", { extensions: [".PNG"] })), ["fig.png"]);

// Without extensions every file is offered.
assert.equal(pathOptions(files, "chapters").length, 5);

// dropExtension omits the extension, as \input does.
assert.deepEqual(labels(pathOptions(files, "chapters", { extensions: ["tex"], dropExtension: true })), ["../main", "intro"]);

// exclude removes the edited file.
assert.deepEqual(labels(pathOptions(files, "chapters", { extensions: ["tex"], exclude: "chapters/intro.tex" })), ["../main.tex"]);

// Options are sorted by label and typed as files.
{
  const options = pathOptions(files, "");
  assert.deepEqual(labels(options), [...labels(options)].sort());
  assert.ok(options.every((option) => option.type === "file"));
}

// Empty input gives no options.
assert.deepEqual(pathOptions([], "chapters"), []);
