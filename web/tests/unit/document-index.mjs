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
  metadata,
  numbering,
  proseRanges,
  references,
  rename,
  renameProblem,
  symbols,
  targetKind,
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
  // An image named without an extension tries the image extensions, not .tex.
  assert.equal(project.resolveFile("main.tex", "plot", "asset"), "plot.png");
  assert.equal(project.resolveFile("main.tex", "plot"), null);
  assert.equal(project.resolveFile("sub/a.tex", "/plot.png", "asset"), "plot.png");
  // An image found only in another folder, as with \graphicspath, resolves when unique.
  project.setFiles(["main.tex", "figures/curve.pdf", "a/twin.png", "b/twin.png"]);
  assert.equal(project.resolveFile("main.tex", "curve", "asset"), "figures/curve.pdf");
  assert.equal(project.resolveFile("main.tex", "twin", "asset"), null);
  assert.equal(project.resolveFile("main.tex", "curve"), null);
  project.setFiles(["main.tex", "intro.tex", "typ.typ", "plot.png"]);

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

test("LaTeX includes resolve from the project root, then beside the including file", () => {
  const project = createProjectIndex();
  project.setFiles(["main.tex", "chapters/ch1.tex", "chapters/sec1.tex", "chapters/figs/a.png", "figs/b.png"]);
  assert.equal(project.resolveFile("chapters/ch1.tex", "chapters/sec1"), "chapters/sec1.tex");
  // Beside the including file still works, as opening.js allows.
  assert.equal(project.resolveFile("chapters/ch1.tex", "sec1"), "chapters/sec1.tex");
  assert.equal(project.resolveFile("chapters/ch1.tex", "figs/b", "asset"), "figs/b.png");
  assert.equal(project.resolveFile("chapters/ch1.tex", "figs/a.png", "asset"), "chapters/figs/a.png");
  assert.equal(project.resolveFile("chapters/ch1.tex", "nope"), null);
});

test("a LaTeX missing-file quick fix suggests the root-relative path", () => {
  const text = "\\input{chapters/sec}\n";
  const project = createProjectIndex();
  project.setFiles(["main.tex", "chapters/ch1.tex", "chapters/sec1.tex"]);
  project.update({
    path: "chapters/ch1.tex",
    format: "latex",
    length: text.length,
    items: [{ kind: "include", range: [0, text.length - 1], name: "chapters/sec", nameRange: [7, 19] }],
  });
  const [found] = diagnostics(project, "chapters/ch1.tex");
  assert.equal(found.code, "missing-file");
  assert.deepEqual(found.suggestions, ["chapters/sec1"]);
});

test("labels scope to their format family, and HTML fragments to their page", () => {
  const project = createProjectIndex();
  project.update({ path: "a.html", format: "html", length: 2, items: [{ kind: "label", range: [0, 1], name: "x", nameRange: [0, 1] }] });
  project.update({ path: "b.html", format: "html", length: 1, items: [{ kind: "ref", range: [0, 1], name: "x", nameRange: [0, 1], namespace: "label" }] });
  project.update({ path: "c.typ", format: "typst", length: 1, items: [{ kind: "ref", range: [0, 1], name: "x", nameRange: [0, 1], namespace: "either" }] });
  project.update({ path: "d.tex", format: "latex", length: 1, items: [{ kind: "label", range: [0, 1], name: "x", nameRange: [0, 1] }] });
  // b.html's #x has no x on b.html, though a.html has an id x.
  assert.deepEqual(definition(project, "b.html", 0), []);
  assert.deepEqual(diagnostics(project, "b.html").map((d) => d.code), ["unknown-label"]);
  // A Typst @x is not a label ref just because HTML or LaTeX define x.
  assert.equal(project.namespaceOf(project.file("c.typ").items[0], "c.typ"), "cite");
  assert.deepEqual(project.definitions("label", "x", "c.typ"), []);
  assert.deepEqual(project.definitions("label", "x", "a.html").map((d) => d.path), ["a.html"]);
  assert.deepEqual(project.names("label", "d.tex"), ["x"]);
  assert.deepEqual(project.names("label", "c.typ"), []);
  // Renaming the id on a.html leaves b.html's fragment alone.
  assert.deepEqual(rename(project, "a.html", 0, "y"), [{ path: "a.html", from: 0, to: 1, insert: "y" }]);
});

test("rename validates the new name per format and refuses a name that exists", () => {
  const project = createProjectIndex();
  const add = (path, format, names) =>
    project.update({
      path,
      format,
      length: 100,
      items: names.map((name, i) => ({ kind: "label", range: [i * 10, i * 10 + 5], name, nameRange: [i * 10, i * 10 + 5] })),
    });
  add("a.tex", "latex", ["fig:a", "fig:taken"]);
  add("a.typ", "typst", ["eq:a"]);
  add("a.qmd", "quarto", ["fig-a", "intro"]);
  add("a.html", "html", ["top"]);
  assert.equal(renameProblem(project, "a.tex", 0, "fig:b"), null);
  for (const bad of ["a{b", "a}b", "a%b", "a,b", "a\\b", "a#b"]) assert.equal(renameProblem(project, "a.tex", 0, bad), "chars", bad);
  assert.equal(renameProblem(project, "a.tex", 0, "fig:taken"), "exists");
  assert.equal(rename(project, "a.tex", 0, "fig:taken"), null);
  assert.equal(renameProblem(project, "a.tex", 0, "fig a"), "invalid");
  assert.equal(renameProblem(project, "a.typ", 0, "eq:b"), null);
  assert.equal(renameProblem(project, "a.typ", 0, "eq b!"), "invalid");
  assert.equal(renameProblem(project, "a.typ", 0, "eq$b"), "chars");
  assert.equal(renameProblem(project, "a.qmd", 0, "fig-b"), null);
  assert.equal(renameProblem(project, "a.qmd", 0, "tbl-b"), "chars", "fig- keeps its prefix");
  assert.equal(renameProblem(project, "a.qmd", 0, "fig-"), "chars");
  assert.equal(renameProblem(project, "a.qmd", 10, "tbl-b"), null, "a plain id may change");
  assert.equal(renameProblem(project, "a.html", 0, 'a"b'), "chars");
  assert.equal(renameProblem(project, "a.html", 0, "bottom"), null);
  // A name taken in another format family is not a collision.
  assert.equal(renameProblem(project, "a.typ", 0, "top"), null);
});

test("label lookups follow updates and removals", () => {
  const project = createProjectIndex();
  const file = (path, name) => ({ path, format: "latex", length: 5, items: [{ kind: "label", range: [0, 5], name, nameRange: [0, 5] }] });
  project.update(file("a.tex", "one"));
  assert.deepEqual(project.names("label"), ["one"]);
  project.update(file("b.tex", "two"));
  assert.deepEqual(project.names("label"), ["one", "two"]);
  project.update(file("a.tex", "three"));
  assert.equal(project.definitions("label", "one").length, 0);
  assert.equal(project.definitions("label", "three").length, 1);
  project.remove("b.tex");
  assert.deepEqual(project.names("label"), ["three"]);
});

// Footnotes, links, metadata, target kinds and the diagnostics added with them.

const fnText = "Note[^x] and {#x} and @x.\n\n[^x]: The body.\n";
const fnRef = at(fnText, "[^x]");
const fnRefName = at(fnText, "x", fnRef[0]);
const fnLabel = at(fnText, "{#x}");
const fnLabelName = at(fnText, "x", fnLabel[0]);
const fnLabelRef = at(fnText, "@x");
const fnLabelRefName = at(fnText, "x", fnLabelRef[0]);
const fnDef = at(fnText, "[^x]: The body.");
const fnDefName = at(fnText, "x", fnDef[0]);

const fnItems = [
  { kind: "ref", range: fnRef, nameRange: fnRefName, name: "x", namespace: "footnote" },
  { kind: "label", range: fnLabel, nameRange: fnLabelName, name: "x", namespace: "label" },
  { kind: "ref", range: fnLabelRef, nameRange: fnLabelRefName, name: "x", namespace: "label" },
  { kind: "label", range: fnDef, nameRange: fnDefName, name: "x", namespace: "footnote", attrs: { target: "footnote" } },
];

/** A Markdown notes file with one footnote and one label, both named x. */
function notesProject() {
  const project = createProjectIndex();
  project.setFiles(["notes.md", "other.md"]);
  project.update({ path: "notes.md", format: "markdown", length: fnText.length, items: fnItems });
  return project;
}

test("footnotes and labels with the same name stay in separate namespaces", () => {
  const project = notesProject();
  assert.equal(project.namespaceOf(fnItems[0], "notes.md"), "footnote");
  assert.equal(project.namespaceOf(fnItems[1], "notes.md"), "label");
  assert.equal(project.namespaceOf(fnItems[2], "notes.md"), "label");
  assert.equal(project.namespaceOf(fnItems[3], "notes.md"), "footnote");
  assert.deepEqual(project.names("footnote"), ["x"]);
  assert.deepEqual(project.names("label"), ["x"]);
  assert.deepEqual(project.definitions("footnote", "x").map(({ item }) => item), [fnItems[3]]);
  assert.deepEqual(project.definitions("label", "x").map(({ item }) => item), [fnItems[1]]);
  assert.deepEqual(project.references("footnote", "x").map(({ item }) => item), [fnItems[0]]);
  assert.deepEqual(project.references("label", "x").map(({ item }) => item), [fnItems[2]]);
  assert.deepEqual(project.definitions("cite", "x"), []);
});

test("a footnote ref goes to its footnote and a label ref to its label", () => {
  const project = notesProject();
  assert.deepEqual(definition(project, "notes.md", fnRefName[0]), [
    { path: "notes.md", from: fnDefName[0], to: fnDefName[1] },
  ]);
  assert.deepEqual(definition(project, "notes.md", fnLabelRefName[0]), [
    { path: "notes.md", from: fnLabelName[0], to: fnLabelName[1] },
  ]);
  assert.deepEqual(references(project, "notes.md", fnDefName[0]), [
    { path: "notes.md", from: fnRefName[0], to: fnRefName[1] },
    { path: "notes.md", from: fnDefName[0], to: fnDefName[1] },
  ]);
});

test("footnotes resolve only within their own file", () => {
  const project = notesProject();
  project.update({
    path: "other.md",
    format: "markdown",
    length: 4,
    items: [{ kind: "ref", range: [0, 4], nameRange: [2, 3], name: "x", namespace: "footnote" }],
  });
  assert.deepEqual(project.definitions("footnote", "x", "other.md"), []);
  assert.deepEqual(definition(project, "other.md", 2), []);
  assert.deepEqual(project.definitions("footnote", "x").map(({ path }) => path), ["notes.md"]);
  assert.deepEqual(project.names("footnote", "other.md"), []);
  assert.deepEqual(diagnostics(project, "other.md").map((d) => [d.code, d.name, d.severity]), [
    ["unknown-footnote", "x", "warning"],
  ]);
  assert.deepEqual(diagnostics(project, "notes.md"), []);
});

test("rename of a footnote rewrites only the bare name at each site", () => {
  const project = notesProject();
  assert.equal(renameProblem(project, "notes.md", fnRefName[0], "note"), null);
  assert.equal(renameProblem(project, "notes.md", fnDefName[0], "note"), null);
  assert.equal(renameProblem(project, "notes.md", fnRefName[0], "a b"), "invalid");
  assert.deepEqual(rename(project, "notes.md", fnRefName[0], "note"), [
    { path: "notes.md", from: fnRefName[0], to: fnRefName[1], insert: "note" },
    { path: "notes.md", from: fnDefName[0], to: fnDefName[1], insert: "note" },
  ]);
  // Renaming the label leaves the footnote's name alone.
  assert.deepEqual(rename(project, "notes.md", fnLabelRefName[0], "y"), [
    { path: "notes.md", from: fnLabelName[0], to: fnLabelName[1], insert: "y" },
    { path: "notes.md", from: fnLabelRefName[0], to: fnLabelRefName[1], insert: "y" },
  ]);
});

test("an Org headline label cannot be renamed, but a footnote labelled form footnote can", () => {
  const project = createProjectIndex();
  const text = "* Intro\n[fn:a]\n";
  project.update({
    path: "org.org",
    format: "org",
    length: text.length,
    items: [
      { kind: "label", range: [0, 7], nameRange: [2, 7], name: "Intro", attrs: { form: "heading", implicit: "true" } },
      { kind: "label", range: [8, 14], nameRange: [10, 11], name: "a", namespace: "footnote", attrs: { target: "footnote", form: "footnote" } },
    ],
  });
  assert.equal(renameProblem(project, "org.org", 3, "Other"), "invalid");
  assert.equal(renameProblem(project, "org.org", 10, "b"), null);
});

test("hover gives the target kind, and the footnote a ref points to", () => {
  const project = notesProject();
  assert.deepEqual(hover(project, "notes.md", fnRefName[0]), {
    name: "x",
    namespace: "footnote",
    targets: [{ path: "notes.md", from: fnDefName[0], to: fnDefName[1], kind: "footnote" }],
  });
  assert.deepEqual(hover(project, "notes.md", fnLabelRefName[0]), {
    name: "x",
    namespace: "label",
    targets: [{ path: "notes.md", from: fnLabelName[0], to: fnLabelName[1] }],
  });

  const figs = createProjectIndex();
  const figText = "{#fig-a}\n@fig-a\n";
  figs.update({
    path: "figs.md",
    format: "markdown",
    length: figText.length,
    items: [
      { kind: "label", range: [0, 8], nameRange: [2, 7], name: "fig-a", namespace: "label", attrs: { target: "figure" }, title: "A plot" },
      { kind: "ref", range: [9, 15], nameRange: [10, 15], name: "fig-a", namespace: "label" },
    ],
  });
  assert.deepEqual(hover(figs, "figs.md", 10), {
    name: "fig-a",
    namespace: "label",
    targets: [{ path: "figs.md", from: 2, to: 7, title: "A plot", kind: "figure" }],
  });
});

test("targetKind reads a label's target, and metadata groups values by name", () => {
  assert.equal(targetKind({ kind: "label", range: [0, 1], attrs: { target: "table" } }), "table");
  assert.equal(targetKind({ kind: "label", range: [0, 1] }), null);

  const project = createProjectIndex();
  const title = { kind: "metadata", range: [0, 10], name: "title", title: "Essay" };
  const author1 = { kind: "metadata", range: [11, 20], name: "author", title: "Ada" };
  const author2 = { kind: "metadata", range: [21, 30], name: "author", title: "Grace" };
  project.update({ path: "doc.md", format: "markdown", length: 30, items: [author2, title, author1, { kind: "heading", range: [0, 1], level: 1, title: "X" }] });
  assert.deepEqual(metadata(project, "doc.md"), { title: [title], author: [author1, author2] });
  assert.deepEqual(metadata(project, "missing.md"), {});
});

test("file links resolve like includes, and external links stay out of definitions and diagnostics", () => {
  const project = createProjectIndex();
  project.setFiles(["notes/a.md", "notes/b.md", "img/x.png"]);
  const text = "[b](b.md#sec) [site](https://e.org) [gone](gone.md)";
  const fileLink = at(text, "b.md#sec");
  const siteLink = at(text, "https://e.org");
  const goneLink = at(text, "gone.md");
  project.update({
    path: "notes/a.md",
    format: "markdown",
    length: text.length,
    items: [
      { kind: "link", range: fileLink, nameRange: [fileLink[0], fileLink[0] + 4], name: "b.md", attrs: { file: "true", fragment: "sec" } },
      { kind: "link", range: siteLink, nameRange: siteLink, name: "https://e.org", attrs: { external: "true" } },
      { kind: "link", range: goneLink, nameRange: goneLink, name: "gone.md", attrs: { file: "true" } },
    ],
  });
  assert.equal(project.resolveFile("notes/a.md", "b.md", "link"), "notes/b.md");
  assert.equal(project.resolveFile("notes/a.md", "b", "link"), "notes/b.md");
  assert.equal(project.resolveFile("notes/a.md", "../img/x.png", "link"), "img/x.png");
  assert.deepEqual(definition(project, "notes/a.md", fileLink[0]), [{ path: "notes/b.md", from: 0, to: 0 }]);
  assert.deepEqual(definition(project, "notes/a.md", siteLink[0]), []);
  assert.deepEqual(diagnostics(project, "notes/a.md").map((d) => [d.code, d.name, d.severity]), [
    ["missing-file", "gone.md", "warning"],
  ]);
});

test("symbols list labels but not footnotes", () => {
  const project = notesProject();
  assert.deepEqual(symbols(project, "notes.md").labels.map((item) => item.name), ["x"]);
  assert.equal(symbols(project, "notes.md").labels[0], fnItems[1]);
});

test("duplicate labels are reported in each file, but implicit labels and footnotes in other files are not", () => {
  const project = createProjectIndex();
  const label = (text) => ({ kind: "label", range: [0, text.length], nameRange: [7, 12], name: "fig:a" });
  project.update({ path: "a.tex", format: "latex", length: 14, items: [label("\\label{fig:a}")] });
  project.update({ path: "b.tex", format: "latex", length: 14, items: [label("\\label{fig:a}")] });
  project.update({ path: "c.tex", format: "latex", length: 4, items: [{ kind: "label", range: [0, 4], nameRange: [0, 4], name: "top", attrs: { implicit: "true" } }] });
  project.update({ path: "d.tex", format: "latex", length: 4, items: [{ kind: "label", range: [0, 4], nameRange: [0, 4], name: "top" }] });
  project.update({ path: "n.md", format: "markdown", length: 10, items: [{ kind: "label", range: [0, 5], nameRange: [2, 3], name: "k", namespace: "footnote" }] });
  project.update({ path: "m.md", format: "markdown", length: 10, items: [{ kind: "label", range: [0, 5], nameRange: [2, 3], name: "k", namespace: "footnote" }] });
  assert.deepEqual(diagnostics(project, "a.tex").map((d) => [d.code, d.severity, d.name]), [["duplicate-label", "warning", "fig:a"]]);
  assert.deepEqual(diagnostics(project, "b.tex").map((d) => d.code), ["duplicate-label"]);
  assert.deepEqual(diagnostics(project, "c.tex"), []);
  assert.deepEqual(diagnostics(project, "d.tex"), []);
  assert.deepEqual(diagnostics(project, "n.md").map((d) => d.code), ["unused-footnote"]);
});

test("unused notes and unused figure-like targets are info diagnostics", () => {
  const text = "{#a}\n{#b}\n{#c}\n{#d}\n@b\n";
  const nameA = at(text, "a");
  const nameB = at(text, "b");
  const nameC = at(text, "c");
  const nameD = at(text, "d");
  const refB = at(text, "@b");
  const project = createProjectIndex();
  project.update({
    path: "t.md",
    format: "markdown",
    length: text.length,
    items: [
      { kind: "label", range: [0, 4], nameRange: nameA, name: "a", attrs: { target: "figure" } },
      { kind: "label", range: [5, 9], nameRange: nameB, name: "b", attrs: { target: "table" } },
      { kind: "label", range: [10, 14], nameRange: nameC, name: "c", attrs: { target: "section" } },
      { kind: "label", range: [15, 19], nameRange: nameD, name: "d", attrs: { target: "equation", implicit: "true" } },
      { kind: "ref", range: refB, nameRange: [refB[0] + 1, refB[1]], name: "b", namespace: "label" },
    ],
  });
  assert.deepEqual(diagnostics(project, "t.md"), [
    { path: "t.md", from: nameA[0], to: nameA[1], severity: "info", code: "unused-target", name: "a", suggestions: [] },
  ]);
});

test("a bibliography in a project with no citations is an info diagnostic", () => {
  const text = "\\bibliography{refs}\n";
  const bib = at(text, "refs");
  const project = createProjectIndex();
  project.update({
    path: "main.tex",
    format: "latex",
    length: text.length,
    items: [{ kind: "bibliography", range: [0, text.length], nameRange: bib, name: "refs" }],
  });
  assert.deepEqual(diagnostics(project, "main.tex").map((d) => [d.code, d.severity]), [["unused-bibliography", "info"]]);
  project.update({
    path: "intro.tex",
    format: "latex",
    length: 6,
    items: [{ kind: "cite", range: [0, 6], nameRange: [0, 6], name: "smith" }],
  });
  assert.deepEqual(diagnostics(project, "main.tex"), []);
});

/** A label item for the numbering tests: `target` is its kind, `offset` its position. */
function labelItem(name, target, offset, attrs = {}) {
  return { kind: "label", range: [offset, offset + 1], nameRange: [offset, offset + 1], name, namespace: "label", attrs: { target, ...attrs } };
}

/** An include item naming `name` at `offset`. */
function includeItem(name, offset) {
  return { kind: "include", range: [offset, offset + 1], nameRange: [offset, offset + 1], name };
}

test("numbering counts each kind in reading order, and theorems share one sequence", () => {
  const project = createProjectIndex();
  project.update({
    path: "paper.tex",
    format: "latex",
    length: 100,
    items: [
      labelItem("fig:a", "figure", 0),
      labelItem("tab:a", "table", 10),
      labelItem("eq:a", "equation", 20),
      labelItem("lst:a", "listing", 30),
      labelItem("thm:a", "theorem", 40),
      labelItem("sec:a", "section", 50),
      labelItem("fig:b", "figure", 60),
      labelItem("thm:b", "theorem", 70),
      labelItem("tab:b", "table", 80),
    ],
  });
  assert.deepEqual(Object.fromEntries(numbering(project)), {
    "label:fig:a": 1,
    "label:tab:a": 1,
    "label:eq:a": 1,
    "label:lst:a": 1,
    "label:thm:a": 1,
    "label:fig:b": 2,
    "label:thm:b": 2,
    "label:tab:b": 2,
  });
});

test("an include in the middle of a file continues the numbers, and the file resumes after it", () => {
  const project = createProjectIndex();
  project.setFiles(["main.tex", "chap.tex"]);
  project.update({
    path: "main.tex",
    format: "latex",
    length: 40,
    items: [labelItem("fig:a", "figure", 0), includeItem("chap.tex", 10), labelItem("fig:c", "figure", 20), includeItem("gone.tex", 30)],
  });
  project.update({
    path: "chap.tex",
    format: "latex",
    length: 20,
    items: [labelItem("fig:b", "figure", 0), labelItem("tab:b", "table", 5), labelItem("fig:bb", "figure", 10)],
  });
  // chap.tex is included, so main.tex is the only root. A missing include adds nothing.
  assert.deepEqual(Object.fromEntries(numbering(project)), {
    "label:fig:a": 1,
    "label:fig:b": 2,
    "label:fig:bb": 3,
    "label:fig:c": 4,
    "label:tab:b": 1,
  });
});

test("cycles end the walk, and a file included twice is counted at its first inclusion", () => {
  const cycle = createProjectIndex();
  cycle.setFiles(["a.tex", "b.tex"]);
  cycle.update({
    path: "a.tex",
    format: "latex",
    length: 30,
    items: [labelItem("fig:a", "figure", 0), includeItem("b.tex", 10), includeItem("b.tex", 20)],
  });
  cycle.update({
    path: "b.tex",
    format: "latex",
    length: 10,
    items: [labelItem("fig:b", "figure", 0), includeItem("a.tex", 5)],
  });
  // Neither file is a root, so the first unreached file starts a walk; the second include of b.tex adds nothing.
  assert.deepEqual(Object.fromEntries(numbering(cycle)), { "label:fig:a": 1, "label:fig:b": 2 });

  const twice = createProjectIndex();
  twice.setFiles(["main.tex", "part.tex"]);
  twice.update({
    path: "main.tex",
    format: "latex",
    length: 30,
    items: [includeItem("part.tex", 0), includeItem("part.tex", 10), labelItem("fig:after", "figure", 20)],
  });
  twice.update({ path: "part.tex", format: "latex", length: 5, items: [labelItem("fig:p", "figure", 0)] });
  assert.deepEqual(Object.fromEntries(numbering(twice)), { "label:fig:p": 1, "label:fig:after": 2 });
});

test("each root is numbered separately, and a shared file keeps the first root's number", () => {
  const project = createProjectIndex();
  project.setFiles(["r1.tex", "r2.tex", "shared.tex"]);
  project.update({
    path: "r1.tex",
    format: "latex",
    length: 20,
    items: [labelItem("fig:x", "figure", 0), includeItem("shared.tex", 10)],
  });
  project.update({
    path: "r2.tex",
    format: "latex",
    length: 20,
    items: [includeItem("shared.tex", 0), labelItem("fig:y", "figure", 10)],
  });
  project.update({ path: "shared.tex", format: "latex", length: 5, items: [labelItem("fig:s", "figure", 0)] });
  // fig:s is the second figure of r1 and keeps that number. r2 counts it first in its own
  // walk, so fig:y is r2's second figure, the same number as fig:s.
  assert.deepEqual(Object.fromEntries(numbering(project)), { "label:fig:x": 1, "label:fig:s": 2, "label:fig:y": 2 });
});

test("implicit labels and labels of other targets take no number", () => {
  const project = createProjectIndex();
  project.update({
    path: "doc.md",
    format: "markdown",
    length: 30,
    items: [
      labelItem("fig-heading", "figure", 0, { implicit: "true" }),
      labelItem("fig:e", "figure", 10),
      labelItem("fn:x", "footnote", 20),
    ],
  });
  assert.deepEqual(Object.fromEntries(numbering(project)), { "label:fig:e": 1 });
});
