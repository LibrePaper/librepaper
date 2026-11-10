import assert from "node:assert/strict";
import test from "node:test";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import { indexDiagnosticsFor } from "../../src/lib/document-index-extensions/index-diagnostics.js";

/** Range of the first `needle` at or after `from`. */
function at(text, needle, from = 0) {
  const start = text.indexOf(needle, from);
  assert.notEqual(start, -1, `missing ${needle}`);
  return [start, start + needle.length];
}

const main = [
  "\\label{fig:a}",
  "See \\ref{fig:a} and \\ref{fig:zz}.",
  "\\cite{smith} \\cite{nobody} @smithx",
  "\\input{intr}",
  "\\includegraphics{plot.png}",
  "",
].join("\n");

const labelA = at(main, "\\label{fig:a}");
const nameLabelA = at(main, "fig:a", labelA[0]);
const refA = at(main, "\\ref{fig:a}");
const nameRefA = at(main, "fig:a", refA[0]);
const refZz = at(main, "\\ref{fig:zz}");
const nameRefZz = at(main, "fig:zz", refZz[0]);
const citeSmith = at(main, "\\cite{smith}");
const nameSmith = at(main, "smith", citeSmith[0]);
const citeNobody = at(main, "\\cite{nobody}");
const nameNobody = at(main, "nobody", citeNobody[0]);
const typstRef = at(main, "@smithx");
const nameTypstRef = at(main, "smithx", typstRef[0]);
const include = at(main, "\\input{intr}");
const nameInclude = at(main, "intr", include[0]);
const asset = at(main, "\\includegraphics{plot.png}");
const nameAsset = at(main, "plot.png", asset[0]);

/** A FileIndex for `text`, with the given items. */
function fileIndex(path, text, items) {
  return { path, format: "latex", items, length: text.length };
}

function projectFor() {
  const project = createProjectIndex();
  project.setFiles(["main.tex", "intro.tex", "plot.png"]);
  project.setBibliography(["smith"]);
  project.update(fileIndex("main.tex", main, [
    { kind: "label", range: labelA, nameRange: nameLabelA, name: "fig:a", namespace: "label" },
    { kind: "ref", range: refA, nameRange: nameRefA, name: "fig:a", namespace: "label" },
    { kind: "ref", range: refZz, nameRange: nameRefZz, name: "fig:zz", namespace: "label" },
    { kind: "cite", range: citeSmith, nameRange: nameSmith, name: "smith" },
    { kind: "cite", range: citeNobody, nameRange: nameNobody, name: "nobody" },
    { kind: "ref", range: typstRef, nameRange: nameTypstRef, name: "smithx", namespace: "either" },
    { kind: "include", range: include, nameRange: nameInclude, name: "intr" },
    { kind: "asset", range: asset, nameRange: nameAsset, name: "plot.png" },
  ]));
  return project;
}

function contextFor(project) {
  return {
    project: () => project,
    path: () => "main.tex",
    open() {},
    applyEdits() {},
  };
}

test("unknown label is reported with its range, message and one action per suggestion", () => {
  const context = contextFor(projectFor());
  const found = indexDiagnosticsFor(context);
  const label = found.find((d) => d.from === nameRefZz[0]);
  assert.ok(label, "expected a diagnostic for fig:zz");
  assert.equal(label.to, nameRefZz[1]);
  assert.equal(label.severity, "warning");
  assert.equal(label.source, "librepaper-index");
  assert.ok(label.message.includes("fig:zz"));
  assert.ok(label.actions.length >= 1);
  assert.ok(label.actions.every((action) => typeof action.name === "string"));
  // fig:zz is within edit distance of fig:a, the only defined label.
  assert.equal(label.actions.length, 1);
});

test("missing include is reported with its range and a suggestion", () => {
  const context = contextFor(projectFor());
  const found = indexDiagnosticsFor(context);
  const missing = found.find((d) => d.from === nameInclude[0]);
  assert.ok(missing, "expected a diagnostic for intr");
  assert.equal(missing.to, nameInclude[1]);
  assert.ok(missing.message.includes("intr"));
  assert.ok(missing.actions.length >= 1);
  assert.ok(missing.actions.some((action) => action.name.includes("intro")));
});

test("the only diagnostics are the unknown label and the missing include", () => {
  const context = contextFor(projectFor());
  const found = indexDiagnosticsFor(context);
  assert.equal(found.length, 2);
});

test("unknown citations and unknown references are not reported", () => {
  const context = contextFor(projectFor());
  const found = indexDiagnosticsFor(context);
  for (const range of [nameNobody, nameTypstRef, nameSmith, nameRefA, nameAsset]) {
    assert.equal(found.some((d) => d.from === range[0]), false);
  }
});

test("a diagnostic that starts past the document length is dropped", () => {
  const context = contextFor(projectFor());
  const found = indexDiagnosticsFor(context, nameInclude[0]);
  assert.equal(found.some((d) => d.from === nameInclude[0]), false);
  assert.ok(found.some((d) => d.from === nameRefZz[0]));
});

test("a diagnostic that runs past the document length is clamped to it", () => {
  const context = contextFor(projectFor());
  const docLength = nameRefZz[0] + 3;
  const found = indexDiagnosticsFor(context, docLength);
  const label = found.find((d) => d.from === nameRefZz[0]);
  assert.ok(label, "expected the clamped diagnostic to remain");
  assert.equal(label.to, docLength);
});

test("without a document length nothing is clamped or dropped", () => {
  const context = contextFor(projectFor());
  const found = indexDiagnosticsFor(context);
  const label = found.find((d) => d.from === nameRefZz[0]);
  assert.equal(label.to, nameRefZz[1]);
});

test("applying an action dispatches a replacement of the range", () => {
  const context = contextFor(projectFor());
  const found = indexDiagnosticsFor(context);
  const missing = found.find((d) => d.from === nameInclude[0]);
  const calls = [];
  const view = { dispatch(spec) { calls.push(spec); } };
  const action = missing.actions.find((a) => a.name.includes("intro"));
  action.apply(view, missing.from, missing.to);
  assert.deepEqual(calls, [
    { changes: { from: nameInclude[0], to: nameInclude[1], insert: "intro" } },
  ]);
});

/** A lint context for `path` in `project`, as the editor supplies it. */
function contextAt(project, path) {
  return { project: () => project, path: () => path, open() {}, applyEdits() {} };
}

test("an unknown footnote is a warning, with the nearest footnote as a quick fix", () => {
  const text = "A[^nte] and B[^note].\n\n[^note]: body\n";
  const nte = at(text, "nte");
  const noteRef = at(text, "[^note]");
  const defRange = at(text, "[^note]: body");
  const noteDef = at(text, "note", defRange[0]);
  const project = createProjectIndex();
  project.setFiles(["fn.md"]);
  project.update({
    path: "fn.md",
    format: "markdown",
    length: text.length,
    items: [
      { kind: "ref", range: at(text, "[^nte]"), nameRange: nte, name: "nte", namespace: "footnote" },
      { kind: "ref", range: noteRef, nameRange: at(text, "note", noteRef[0]), name: "note", namespace: "footnote" },
      { kind: "label", range: defRange, nameRange: noteDef, name: "note", namespace: "footnote", attrs: { target: "footnote" } },
    ],
  });
  const found = indexDiagnosticsFor(contextAt(project, "fn.md"));
  assert.equal(found.length, 1);
  const [diagnostic] = found;
  assert.equal(diagnostic.from, nte[0]);
  assert.equal(diagnostic.to, nte[1]);
  assert.equal(diagnostic.severity, "warning");
  assert.ok(diagnostic.message.includes("nte"));
  assert.equal(diagnostic.actions.length, 1);
  assert.ok(diagnostic.actions[0].name.includes("note"));
  const calls = [];
  diagnostic.actions[0].apply({ dispatch: (spec) => calls.push(spec) }, diagnostic.from, diagnostic.to);
  assert.deepEqual(calls, [{ changes: { from: nte[0], to: nte[1], insert: "note" } }]);
});

test("a duplicate label is reported as a warning on each copy", () => {
  const text = "\\label{fig:a}\n";
  const name = at(text, "fig:a");
  const project = createProjectIndex();
  project.update({ path: "a.tex", format: "latex", length: text.length, items: [{ kind: "label", range: [0, 14], nameRange: name, name: "fig:a" }] });
  project.update({ path: "b.tex", format: "latex", length: text.length, items: [{ kind: "label", range: [0, 14], nameRange: name, name: "fig:a" }] });
  const [diagnostic] = indexDiagnosticsFor(contextAt(project, "b.tex"));
  assert.equal(diagnostic.severity, "warning");
  assert.equal(diagnostic.from, name[0]);
  assert.ok(diagnostic.message.includes("fig:a"));
  assert.deepEqual(diagnostic.actions, []);
});

test("unused footnotes, unused figure targets and unused bibliographies are info, with no quick fix", () => {
  const footText = "[^lone]: never used\n";
  const footName = at(footText, "lone");
  const labelText = "{#fig}\n";
  const labelName = at(labelText, "fig");
  const bibText = "\\bibliography{refs}\n";
  const bibName = at(bibText, "refs");
  const project = createProjectIndex();
  project.update({
    path: "foot.md",
    format: "markdown",
    length: footText.length,
    items: [{ kind: "label", range: [0, 20], nameRange: footName, name: "lone", namespace: "footnote", attrs: { target: "footnote" } }],
  });
  project.update({
    path: "fig.md",
    format: "markdown",
    length: labelText.length,
    items: [{ kind: "label", range: [0, 7], nameRange: labelName, name: "fig", attrs: { target: "figure" } }],
  });
  project.update({
    path: "main.tex",
    format: "latex",
    length: bibText.length,
    items: [{ kind: "bibliography", range: [0, bibText.length], nameRange: bibName, name: "refs" }],
  });

  const footnote = indexDiagnosticsFor(contextAt(project, "foot.md"));
  assert.equal(footnote.length, 1);
  assert.equal(footnote[0].severity, "info");
  assert.equal(footnote[0].source, "librepaper-index");
  assert.ok(footnote[0].message.includes("lone"));
  assert.deepEqual(footnote[0].actions, []);

  const target = indexDiagnosticsFor(contextAt(project, "fig.md"));
  assert.equal(target.length, 1);
  assert.equal(target[0].severity, "info");
  assert.equal(target[0].from, labelName[0]);
  assert.ok(target[0].message.includes("fig"));

  const bibliography = indexDiagnosticsFor(contextAt(project, "main.tex"));
  assert.equal(bibliography.length, 1);
  assert.equal(bibliography[0].severity, "info");
  assert.ok(bibliography[0].message.length > 0);
});

test("a link to a missing project file is a warning with the nearest file as a quick fix", () => {
  const text = "[x](b.mdd)\n";
  const name = at(text, "b.mdd");
  const project = createProjectIndex();
  project.setFiles(["notes/a.md", "notes/b.md"]);
  project.update({
    path: "notes/a.md",
    format: "markdown",
    length: text.length,
    items: [{ kind: "link", range: [4, 9], nameRange: name, name: "b.mdd", attrs: { file: "true" } }],
  });
  const [diagnostic] = indexDiagnosticsFor(contextAt(project, "notes/a.md"));
  assert.equal(diagnostic.severity, "warning");
  assert.ok(diagnostic.message.includes("b.mdd"));
  assert.ok(diagnostic.actions.some((action) => action.name.includes("b.md")));
});
