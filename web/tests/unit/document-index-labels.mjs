// Label lists from the document index: the order of the lists, the first
// definition winning, the format filter, 1-based lines, titles, and which
// files the index of texts skips.
import assert from "node:assert/strict";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import { indexTexts, isDocumentSource, labelList } from "../../src/lib/document-index/labels.js";

const label = (name, from, title) => ({ kind: "label", name, range: [from, from + name.length], ...(title ? { title } : {}) });

// Hand-written items, so the order checks do not depend on any adapter. b.tex
// lists b2 before b1, to show that the list follows offsets, not item order.
function projectOf(files) {
  const project = createProjectIndex();
  for (const file of files) project.update({ length: 100, ...file });
  return project;
}

// --- order -------------------------------------------------------------------
{
  const project = projectOf([
    { path: "b.tex", format: "latex", items: [label("b2", 20), label("b1", 2)] },
    { path: "a.tex", format: "latex", items: [label("a1", 0)] },
    { path: "main.tex", format: "latex", items: [label("m1", 0)] },
  ]);
  // Path order by default.
  assert.deepEqual(labelList(project, {}).map((entry) => entry.id), ["a1", "b1", "b2", "m1"]);
  // The first file leads, then the rest in path order.
  assert.deepEqual(labelList(project, {}, { first: "main.tex" }).map((entry) => entry.id), ["m1", "a1", "b1", "b2"]);
  assert.deepEqual(labelList(project, {}, { first: "b.tex" }).map((entry) => entry.id), ["b1", "b2", "a1", "m1"]);
  // A first file that is not in the project changes nothing.
  assert.deepEqual(labelList(project, {}, { first: "nope.tex" }).map((entry) => entry.id), ["a1", "b1", "b2", "m1"]);
}

// --- first definition wins ---------------------------------------------------
{
  const project = projectOf([
    { path: "a.tex", format: "latex", items: [label("dup", 5)] },
    { path: "main.tex", format: "latex", items: [label("dup", 9), label("only", 30)] },
  ]);
  const texts = { "a.tex": "\n\n\n\n\ndup", "main.tex": "x".repeat(9) + "dup" };
  assert.deepEqual(labelList(project, texts), [
    { id: "dup", path: "a.tex", line: 6 },
    { id: "only", path: "main.tex", line: 1 },
  ]);
  // With main.tex first, its definition is the one kept.
  assert.deepEqual(labelList(project, texts, { first: "main.tex" }), [
    { id: "dup", path: "main.tex", line: 1 },
    { id: "only", path: "main.tex", line: 1 },
  ]);
}

// --- formats -----------------------------------------------------------------
{
  const project = projectOf([
    { path: "main.tex", format: "latex", items: [label("tex1", 0)] },
    { path: "notes.md", format: "markdown", items: [label("md1", 0)] },
    { path: "deck.typ", format: "typst", items: [label("typ1", 0)] },
  ]);
  assert.deepEqual(labelList(project, {}, { formats: new Set(["latex"]) }).map((entry) => entry.id), ["tex1"]);
  // Path order: deck.typ, main.tex, notes.md.
  assert.deepEqual(labelList(project, {}, { formats: new Set(["markdown", "typst"]) }).map((entry) => entry.id), ["typ1", "md1"]);
  // No filter keeps every format.
  assert.deepEqual(labelList(project, {}).map((entry) => entry.id), ["typ1", "tex1", "md1"]);
}

// --- lines -------------------------------------------------------------------
{
  const project = projectOf([
    { path: "a.tex", format: "latex", items: [label("third", 3)] },
    { path: "b.tex", format: "latex", items: [label("gone", 0)] },
  ]);
  // "x\n\nlab": the label starts after two line breaks, on line 3.
  const texts = { "a.tex": "x\n\nthird" };
  assert.deepEqual(labelList(project, texts), [
    { id: "third", path: "a.tex", line: 3 },
    { id: "gone", path: "b.tex", line: 1 },
  ]);
  // The first line, and a label at offset 0, are line 1.
  assert.equal(labelList(project, { "a.tex": "third" }).find((entry) => entry.id === "third").line, 1);
}

// --- titles ------------------------------------------------------------------
{
  const project = projectOf([
    { path: "a.tex", format: "latex", items: [label("titled", 0, "Results"), label("plain", 10)] },
  ]);
  const [titled, plain] = labelList(project, {});
  assert.equal(titled.title, "Results");
  assert.equal("title" in plain, false);
}

// --- document sources only -----------------------------------------------------
{
  const project = projectOf([
    { path: "main.tex", format: "latex", items: [label("sec:a", 0)] },
    { path: "pkg.sty", format: "latex", items: [label("pkg:a", 0)] },
  ]);
  const texts = { "main.tex": "\\label{sec:a}", "pkg.sty": "\\label{pkg:a}" };
  assert.deepEqual(labelList(project, texts).map((entry) => entry.id), ["sec:a"]);
  assert.deepEqual(labelList(project, texts, { formats: new Set(["latex"]) }).map((entry) => entry.id), ["sec:a"]);
  // The index still holds the style file's label, so it resolves.
  assert.equal(project.definitions("label", "pkg:a").length, 1);
}

// isDocumentSource: the document extensions of formats.js, and not style, class or bibliography files.
{
  for (const path of ["main.tex", "chap/two.ltx", "deck.typ", "notes.md", "notes.markdown", "page.html", "page.htm", "doc.qmd"]) {
    assert.equal(isDocumentSource(path), true, path);
  }
  for (const path of ["pkg.sty", "class.cls", "defs.def", "refs.bib", "fig.pgf", "notes.txt", ""]) {
    assert.equal(isDocumentSource(path), false, path);
  }
}

// --- indexTexts ----------------------------------------------------------------
{
  // .txt and .bib files have no adapter, and a null text is skipped, so only
  // main.tex is in the project.
  const texts = {
    "main.tex": "\\label{sec:a}",
    "notes.txt": "\\label{ignored}",
    "refs.bib": "@article{k}",
    "empty.typ": null,
  };
  const project = indexTexts(texts);
  assert.deepEqual(project.paths(), ["main.tex"]);
  assert.equal(project.file("notes.txt"), undefined);
  assert.equal(project.file("empty.typ"), undefined);
  assert.deepEqual(labelList(project, texts).map((entry) => entry.id), ["sec:a"]);
}

// A typst heading label takes the heading as its title, and its line is read
// from the text the entry is given.
{
  const texts = { "main.typ": "Intro line\n= Intro <sec:intro>\n" };
  const [entry] = labelList(indexTexts(texts), texts, { formats: new Set(["typst"]) });
  assert.equal(entry.id, "sec:intro");
  assert.equal(entry.path, "main.typ");
  assert.equal(entry.line, 2);
  assert.equal(entry.title, "Intro");
}
