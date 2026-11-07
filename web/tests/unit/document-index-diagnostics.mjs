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
