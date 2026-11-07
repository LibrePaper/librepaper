import assert from "node:assert/strict";
import test from "node:test";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import {
  definition,
  diagnostics,
  foldingRanges,
  hover,
  itemAt,
  levenshtein,
  proseRanges,
  references,
  rename,
  symbols,
} from "../../src/lib/document-index/queries.js";

/** Range of the first `needle` at or after `from`. */
function at(text, needle, from = 0) {
  const start = text.indexOf(needle, from);
  assert.notEqual(start, -1, `missing ${needle}`);
  return [start, start + needle.length];
}

const main = "\\section{Intro}\n\\label{fig:a}\nSee \\ref{fig:a} and \\cite{smith}.\n\\input{intro}\n\\section{Methods}\n\\ref{fig:b} \\ref{fig:bb} \\cite{nobody}\n\\input{intr}\n\\includegraphics{plot.png}\n";
const intro = "\\label{fig:b}\nText.\n";
const typ = "= Notes\n<eq:x>\n@eq:x @smith @nowhere\n";

const sec1 = at(main, "\\section{Intro}");
const labelA = at(main, "\\label{fig:a}");
const nameA = at(main, "fig:a", labelA[0]);
const refA = at(main, "\\ref{fig:a}");
const refAName = at(main, "fig:a", refA[0]);
const citeSmith = at(main, "\\cite{smith}");
const smithName = at(main, "smith", citeSmith[0]);
const inc = at(main, "\\input{intro}");
const introName = at(main, "intro", inc[0]);
const sec2 = at(main, "\\section{Methods}");
const refB = at(main, "\\ref{fig:b}");
const refBName = at(main, "fig:b", refB[0]);
const refBB = at(main, "\\ref{fig:bb}");
const refBBName = at(main, "fig:bb", refBB[0]);
const citeNobody = at(main, "\\cite{nobody}");
const nobodyName = at(main, "nobody", citeNobody[0]);
const incBad = at(main, "\\input{intr}");
const incBadName = at(main, "intr", incBad[0]);
const asset = at(main, "\\includegraphics{plot.png}");
const assetName = at(main, "plot.png", asset[0]);

const labelB = at(intro, "\\label{fig:b}");
const nameB = at(intro, "fig:b", labelB[0]);

const heading = at(typ, "= Notes");
const labelX = at(typ, "<eq:x>");
const nameX = at(typ, "eq:x", labelX[0]);
const refX = at(typ, "@eq:x");
const nameRefX = at(typ, "eq:x", refX[0]);
const refSmith = at(typ, "@smith");
const nameSmithTyp = at(typ, "smith", refSmith[0]);
const refNowhere = at(typ, "@nowhere");
const nameNowhere = at(typ, "nowhere", refNowhere[0]);

function buildProject(options) {
  const project = createProjectIndex(options);
  project.update({
    path: "main.tex",
    format: "latex",
    length: main.length,
    items: [
      { kind: "heading", range: sec1, level: 1, title: "Intro" },
      { kind: "label", range: labelA, name: "fig:a", nameRange: nameA, title: "A plot" },
      { kind: "ref", range: refA, name: "fig:a", nameRange: refAName, namespace: "label" },
      { kind: "cite", range: citeSmith, name: "smith", nameRange: smithName },
      { kind: "include", range: inc, name: "intro", nameRange: introName },
      { kind: "heading", range: sec2, level: 1, title: "Methods" },
      { kind: "ref", range: refB, name: "fig:b", nameRange: refBName, namespace: "label" },
      { kind: "ref", range: refBB, name: "fig:bb", nameRange: refBBName, namespace: "label" },
      { kind: "cite", range: citeNobody, name: "nobody", nameRange: nobodyName },
      { kind: "include", range: incBad, name: "intr", nameRange: incBadName },
      { kind: "asset", range: asset, name: "plot.png", nameRange: assetName },
    ],
  });
  project.update({
    path: "intro.tex",
    format: "latex",
    length: intro.length,
    items: [{ kind: "label", range: labelB, name: "fig:b", nameRange: nameB }],
  });
  project.update({
    path: "typ.typ",
    format: "typst",
    length: typ.length,
    items: [
      { kind: "heading", range: heading, level: 1, title: "Notes" },
      { kind: "label", range: labelX, name: "eq:x", nameRange: nameX },
      { kind: "ref", range: refX, name: "eq:x", nameRange: nameRefX, namespace: "either" },
      { kind: "ref", range: refSmith, name: "smith", nameRange: nameSmithTyp, namespace: "either" },
      { kind: "ref", range: refNowhere, name: "nowhere", nameRange: nameNowhere, namespace: "either" },
    ],
  });
  project.setFiles(["main.tex", "intro.tex", "typ.typ", "plot.png"]);
  project.setBibliography(["jones", "smith"]);
  return project;
}

test("the project index stores files by path and removes them", () => {
  const project = createProjectIndex();
  project.update({ path: "b.tex", format: "latex", length: 0, items: [] });
  project.update({ path: "a.tex", format: "latex", length: 0, items: [] });
  assert.deepEqual(project.paths(), ["a.tex", "b.tex"]);
  project.remove("a.tex");
  assert.equal(project.file("a.tex"), undefined);
  assert.deepEqual(project.paths(), ["b.tex"]);
});

test("either refs resolve to a label when one exists, otherwise to a citation", () => {
  const project = buildProject();
  assert.equal(project.namespaceOf({ kind: "ref", range: [0, 1], name: "eq:x", namespace: "either" }), "label");
  assert.equal(project.namespaceOf({ kind: "ref", range: [0, 1], name: "smith", namespace: "either" }), "cite");
  assert.equal(project.namespaceOf({ kind: "ref", range: [0, 1], name: "fig:a" }), "label");
  assert.equal(project.namespaceOf({ kind: "ref", range: [0, 1], name: "smith", namespace: "cite" }), "cite");
  assert.equal(project.namespaceOf({ kind: "cite", range: [0, 1], name: "smith" }), "cite");
  assert.equal(project.namespaceOf({ kind: "label", range: [0, 1], name: "zz" }), "label");
  assert.equal(project.namespaceOf({ kind: "heading", range: [0, 1] }), null);
});

test("names, definitions and citation keys come from the project", () => {
  const project = buildProject();
  assert.deepEqual(project.names("label"), ["eq:x", "fig:a", "fig:b"]);
  assert.deepEqual(project.names("cite"), ["jones", "smith"]);
  assert.deepEqual(project.definitions("label", "fig:b"), [
    { path: "intro.tex", item: { kind: "label", range: labelB, name: "fig:b", nameRange: nameB } },
  ]);
  assert.deepEqual(project.definitions("cite", "smith"), []);
  assert.equal(project.hasCitation("smith"), true);
  assert.equal(project.hasCitation("nope"), false);

  project.setBibliography(null);
  assert.equal(project.hasCitation("smith"), null);
  assert.deepEqual(project.names("cite"), []);
});

test("references collect every ref and cite that resolves to a namespace", () => {
  const project = buildProject();
  assert.deepEqual(project.references("cite", "smith").map(({ path }) => path), ["main.tex", "typ.typ"]);
  assert.deepEqual(references(project, "main.tex", nameA[0]), [
    { path: "main.tex", from: nameA[0], to: nameA[1] },
    { path: "main.tex", from: refAName[0], to: refAName[1] },
  ]);
  assert.deepEqual(references(project, "main.tex", nameA[0], { includeDeclaration: false }), [
    { path: "main.tex", from: refAName[0], to: refAName[1] },
  ]);
});

test("references across files and through either refs", () => {
  const project = buildProject();
  assert.deepEqual(references(project, "intro.tex", nameB[0]), [
    { path: "intro.tex", from: nameB[0], to: nameB[1] },
    { path: "main.tex", from: refBName[0], to: refBName[1] },
  ]);
  assert.deepEqual(references(project, "main.tex", smithName[0]), [
    { path: "main.tex", from: smithName[0], to: smithName[1] },
    { path: "typ.typ", from: nameSmithTyp[0], to: nameSmithTyp[1] },
  ]);
  assert.deepEqual(references(project, "typ.typ", nameRefX[0]), [
    { path: "typ.typ", from: nameX[0], to: nameX[1] },
    { path: "typ.typ", from: nameRefX[0], to: nameRefX[1] },
  ]);
});

test("definition follows refs to labels in other files", () => {
  const project = buildProject();
  assert.deepEqual(definition(project, "main.tex", refBName[0]), [
    { path: "intro.tex", from: nameB[0], to: nameB[1] },
  ]);
  assert.deepEqual(definition(project, "main.tex", smithName[0]), []);
  assert.deepEqual(definition(project, "typ.typ", nameRefX[0]), [
    { path: "typ.typ", from: nameX[0], to: nameX[1] },
  ]);
  assert.deepEqual(definition(project, "typ.typ", nameSmithTyp[0]), []);
  assert.deepEqual(definition(project, "main.tex", 0), []);
});

test("definition of an include uses resolveFile, with extension inference", () => {
  const project = buildProject();
  assert.deepEqual(definition(project, "main.tex", introName[0]), [{ path: "intro.tex", from: 0, to: 0 }]);
  assert.deepEqual(definition(project, "main.tex", assetName[0]), [{ path: "plot.png", from: 0, to: 0 }]);
  assert.deepEqual(definition(project, "main.tex", incBadName[0]), []);

  assert.equal(project.resolveFile("main.tex", "intro"), "intro.tex");
  assert.equal(project.resolveFile("main.tex", "intro.tex"), "intro.tex");
  assert.equal(project.resolveFile("main.tex", "intr"), null);
  assert.equal(project.resolveFile("sub/a.tex", "../main"), "main.tex");
  assert.equal(project.resolveFile("sub/a.tex", "../typ"), null);
  assert.equal(project.resolveFile("sub/a.tex", "../typ.typ"), "typ.typ");

  project.setFiles(null);
  assert.equal(project.resolveFile("main.tex", "intro"), null);

  const overridden = buildProject({ resolveFile: () => "override.tex" });
  assert.deepEqual(definition(overridden, "main.tex", introName[0]), [{ path: "override.tex", from: 0, to: 0 }]);
});

test("itemAt picks the smallest item containing the position", () => {
  const project = createProjectIndex();
  project.update({
    path: "o.tex",
    format: "latex",
    length: 30,
    items: [
      { kind: "include", range: [0, 20], name: "a" },
      { kind: "ref", range: [2, 12], nameRange: [4, 6], name: "x", namespace: "label" },
    ],
  });
  assert.equal(itemAt(project, "o.tex", 5).kind, "ref");
  assert.equal(itemAt(project, "o.tex", 6).kind, "ref");
  assert.equal(itemAt(project, "o.tex", 15).kind, "include");
  assert.equal(itemAt(project, "o.tex", 25), null);
});

test("rename edits every definition and reference of a label", () => {
  const project = buildProject();
  assert.deepEqual(rename(project, "main.tex", nameA[0], "fig:new"), [
    { path: "main.tex", from: nameA[0], to: nameA[1], insert: "fig:new" },
    { path: "main.tex", from: refAName[0], to: refAName[1], insert: "fig:new" },
  ]);
  assert.deepEqual(rename(project, "main.tex", refBName[0], "fig:c"), [
    { path: "intro.tex", from: nameB[0], to: nameB[1], insert: "fig:c" },
    { path: "main.tex", from: refBName[0], to: refBName[1], insert: "fig:c" },
  ]);
  assert.deepEqual(rename(project, "typ.typ", nameRefX[0], "eq:y"), [
    { path: "typ.typ", from: nameX[0], to: nameX[1], insert: "eq:y" },
    { path: "typ.typ", from: nameRefX[0], to: nameRefX[1], insert: "eq:y" },
  ]);
});

test("rename returns null when it cannot apply", () => {
  const project = buildProject();
  assert.equal(rename(project, "main.tex", nameA[0], ""), null);
  assert.equal(rename(project, "main.tex", nameA[0], "fig x"), null);
  assert.equal(rename(project, "main.tex", nameA[0], "fig:a"), null);
  assert.equal(rename(project, "main.tex", smithName[0], "other"), null);
  assert.equal(rename(project, "main.tex", introName[0], "other"), null);
  assert.equal(rename(project, "typ.typ", nameSmithTyp[0], "other"), null);
  assert.equal(rename(project, "main.tex", 0, "other"), null);
});

test("hover shows label titles and citation targets", () => {
  const project = buildProject();
  assert.deepEqual(hover(project, "main.tex", refAName[0]), {
    name: "fig:a",
    namespace: "label",
    targets: [{ path: "main.tex", from: nameA[0], to: nameA[1], title: "A plot" }],
  });
  assert.deepEqual(hover(project, "main.tex", refBName[0]), {
    name: "fig:b",
    namespace: "label",
    targets: [{ path: "intro.tex", from: nameB[0], to: nameB[1] }],
  });
  assert.deepEqual(hover(project, "main.tex", smithName[0]), { name: "smith", namespace: "cite", targets: [] });
  assert.equal(hover(project, "main.tex", nameA[0]), null);
});

test("symbols list headings and labels in source order", () => {
  const project = buildProject();
  const result = symbols(project, "main.tex");
  assert.deepEqual(result.headings.map((item) => item.title), ["Intro", "Methods"]);
  assert.deepEqual(result.labels.map((item) => item.name), ["fig:a"]);
});

test("folding spans nested headings and blocks, dropping empty ranges", () => {
  const project = createProjectIndex();
  project.update({
    path: "nest.tex",
    format: "latex",
    length: 100,
    items: [
      { kind: "heading", range: [0, 10], level: 1 },
      { kind: "heading", range: [20, 30], level: 2 },
      { kind: "heading", range: [40, 50], level: 2 },
      { kind: "heading", range: [60, 70], level: 1 },
      { kind: "heading", range: [75, 80], level: 3 },
      { kind: "block", range: [85, 95], name: "figure" },
      { kind: "block", range: [5, 5], name: "empty" },
    ],
  });
  assert.deepEqual(foldingRanges(project, "nest.tex"), [
    [10, 60],
    [30, 40],
    [50, 60],
    [70, 100],
    [80, 100],
    [85, 95],
  ]);
});

test("unknown labels, citations and references and missing files are reported", () => {
  const project = buildProject();
  assert.deepEqual(diagnostics(project, "main.tex"), [
    {
      path: "main.tex",
      from: refBBName[0],
      to: refBBName[1],
      severity: "warning",
      code: "unknown-label",
      name: "fig:bb",
      suggestions: ["fig:b", "fig:a"],
    },
    {
      path: "main.tex",
      from: nobodyName[0],
      to: nobodyName[1],
      severity: "warning",
      code: "unknown-citation",
      name: "nobody",
      suggestions: [],
    },
    {
      path: "main.tex",
      from: incBadName[0],
      to: incBadName[1],
      severity: "warning",
      code: "missing-file",
      name: "intr",
      suggestions: ["intro"],
    },
  ]);
  assert.deepEqual(diagnostics(project, "intro.tex"), []);
  assert.deepEqual(diagnostics(project, "typ.typ"), [
    {
      path: "typ.typ",
      from: nameNowhere[0],
      to: nameNowhere[1],
      severity: "warning",
      code: "unknown-reference",
      name: "nowhere",
      suggestions: [],
    },
  ]);
});

test("an unknown bibliography and an unknown file list silence those checks", () => {
  const project = buildProject();
  project.setBibliography(null);
  assert.deepEqual(diagnostics(project, "main.tex").map((d) => d.code), ["unknown-label", "missing-file"]);
  assert.deepEqual(diagnostics(project, "typ.typ"), []);

  project.setBibliography(["jones", "smith"]);
  project.setFiles(null);
  assert.deepEqual(diagnostics(project, "main.tex").map((d) => d.code), ["unknown-label", "unknown-citation"]);
});

test("suggestions are limited to names within edit distance 2, closest first", () => {
  const project = createProjectIndex();
  project.update({
    path: "s.tex",
    format: "latex",
    length: 40,
    items: [
      { kind: "label", range: [0, 5], name: "sec:intro" },
      { kind: "label", range: [6, 11], name: "sec:introduction" },
      { kind: "label", range: [12, 17], name: "sec:intr" },
      { kind: "ref", range: [20, 30], name: "sec:intro2", namespace: "label", nameRange: [22, 30] },
    ],
  });
  const [diagnostic] = diagnostics(project, "s.tex");
  assert.equal(diagnostic.code, "unknown-label");
  // "sec:intro" is 1 edit away, "sec:intr" 2, "sec:introduction" too far.
  assert.deepEqual(diagnostic.suggestions, ["sec:intro", "sec:intr"]);
});

test("proseRanges subtracts code, math and skip items, merging overlaps", () => {
  const project = createProjectIndex();
  project.update({
    path: "p.tex",
    format: "latex",
    length: 30,
    items: [
      { kind: "code", range: [5, 10] },
      { kind: "math", range: [8, 15] },
      { kind: "block", range: [12, 14], attrs: { prose: "skip" } },
      { kind: "heading", range: [20, 25], attrs: { prose: "skip" } },
      { kind: "code", range: [22, 28] },
      { kind: "code", range: [3, 3] },
    ],
  });
  assert.deepEqual(proseRanges(project, "p.tex"), [
    [0, 5],
    [15, 20],
    [28, 30],
  ]);
});

test("levenshtein counts single-character edits", () => {
  assert.equal(levenshtein("", ""), 0);
  assert.equal(levenshtein("abc", ""), 3);
  assert.equal(levenshtein("", "ab"), 2);
  assert.equal(levenshtein("kitten", "sitting"), 3);
  assert.equal(levenshtein("fig:bb", "fig:a"), 2);
  assert.equal(levenshtein("fig:a", "fig:bb"), 2);
  assert.equal(levenshtein("same", "same"), 0);
});
