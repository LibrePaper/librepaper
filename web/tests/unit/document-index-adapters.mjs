import assert from "node:assert/strict";
import { test } from "node:test";
import { adapterFor, indexFile } from "../../src/lib/document-index/adapters/index.js";

test("each source format gets its adapter, other files none", () => {
  for (const path of ["a.tex", "a.typ", "a.md", "a.qmd", "a.html"]) assert.equal(typeof adapterFor(path), "function", path);
  assert.equal(adapterFor("data.csv"), null);
  assert.equal(indexFile("data.csv", "x"), null);
});

test("indexFile records the path, format, length and items", () => {
  const text = "\\section{Intro}\\label{sec:intro}\nSee \\ref{sec:intro}.\n";
  const file = indexFile("main.tex", text);
  assert.equal(file?.format, "latex");
  assert.equal(file?.length, text.length);
  assert.ok(file?.items.some((item) => item.kind === "label" && item.name === "sec:intro"));
  assert.ok(file?.items.some((item) => item.kind === "ref" && item.name === "sec:intro"));
});
