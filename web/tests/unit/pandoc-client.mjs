// The path rules and folder layout for Pandoc's in-memory file system. The
// 61 MB module is not loaded here; see tests/integration/pandoc-wasm.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import { OUTPUT_NAME, fileBytes, fillTree, splitPath } from "../../src/lib/pandoc/files.js";

const file = (data) => ({ data });
const folder = (contents) => ({ contents });

test("splitPath returns the segments of a project path", () => {
  assert.deepEqual(splitPath("main.md"), ["main.md"]);
  assert.deepEqual(splitPath("paper/figs/a.png"), ["paper", "figs", "a.png"]);
});

test("splitPath rejects paths that leave the project or are malformed", () => {
  for (const bad of ["", "/abs.md", "a/../b.md", "../b.md", "./b.md", "a//b.md", "a/", "a\\b.md", "a/./b"]) {
    assert.throws(() => splitPath(bad), undefined, JSON.stringify(bad));
  }
  assert.throws(() => splitPath(undefined));
});

test("splitPath rejects the module's own root files", () => {
  for (const name of ["stdin", "stdout", "stderr", "warnings", OUTPUT_NAME]) assert.throws(() => splitPath(name), /reserved/);
  assert.deepEqual(splitPath("docs/stdin"), ["docs", "stdin"]);
});

test("fileBytes encodes strings as UTF-8 and keeps bytes", () => {
  assert.deepEqual(fileBytes("é"), Uint8Array.of(0xc3, 0xa9));
  const bytes = Uint8Array.of(1, 2);
  assert.equal(fileBytes(bytes), bytes);
  assert.throws(() => fileBytes(5));
});

test("fillTree builds nested folders and clears what was there", () => {
  const root = new Map([["old", file(new Uint8Array())]]);
  fillTree(root, { "paper/main.md": "# Hi", "paper/img/a.png": Uint8Array.of(7), "references.bib": "@a{}" }, file, folder);
  assert.deepEqual([...root.keys()].sort(), ["paper", "references.bib"]);
  const paper = root.get("paper").contents;
  assert.deepEqual([...paper.keys()].sort(), ["img", "main.md"]);
  assert.deepEqual(paper.get("img").contents.get("a.png").data, Uint8Array.of(7));
  assert.equal(new TextDecoder().decode(paper.get("main.md").data), "# Hi");
});

test("fillTree rejects a file used as a folder and a folder used as a file", () => {
  assert.throws(() => fillTree(new Map(), { a: "x", "a/b": "y" }, file, folder), /as a folder/);
  assert.throws(() => fillTree(new Map(), { "a/b": "y", a: "x" }, file, folder), /also a folder/);
});
