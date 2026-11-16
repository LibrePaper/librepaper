import assert from "node:assert/strict";
import { test } from "node:test";
import { EditorState } from "@codemirror/state";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
import { findReferencesCommand, referenceRows, references } from "../../src/lib/document-index-extensions/references.js";
import { indexChanged } from "../../src/lib/document-index-extensions/context.js";
import { renameTarget } from "../../src/lib/document-index-extensions/rename.js";
import { m } from "../../src/lib/paraglide/messages.js";

// Two files. main.tex defines sec:a, cites smith2020, and uses both names
// through refs. chap.tex uses sec:a.
const MAIN = "\\label{sec:a}\nSee \\ref{sec:a}, \\cite{smith2020}, \\ref{smith2020}.\n";
const CHAP = "Text \\ref{sec:a} here.\n";

/**
 * A span for `\command{name}`: the whole command, and the name inside the braces.
 * @param {string} text
 * @param {string} command
 * @param {string} name
 */
function spans(text, command, name) {
  const start = text.indexOf(`${command}{${name}}`);
  assert.ok(start >= 0, `${command}{${name}} is in the text`);
  const nameFrom = start + command.length + 1;
  return { range: [start, start + command.length + name.length + 2], nameRange: [nameFrom, nameFrom + name.length] };
}

const mainIndex = {
  path: "main.tex",
  format: "latex",
  length: MAIN.length,
  items: [
    { kind: "label", name: "sec:a", ...spans(MAIN, "\\label", "sec:a") },
    { kind: "ref", name: "sec:a", namespace: "label", ...spans(MAIN, "\\ref", "sec:a") },
    { kind: "cite", name: "smith2020", ...spans(MAIN, "\\cite", "smith2020") },
    { kind: "ref", name: "smith2020", namespace: "either", ...spans(MAIN, "\\ref", "smith2020") },
  ],
};

const chapIndex = {
  path: "chap.tex",
  format: "latex",
  length: CHAP.length,
  items: [{ kind: "ref", name: "sec:a", namespace: "label", ...spans(CHAP, "\\ref", "sec:a") }],
};

/** A project index holding the two files, and a context on main.tex. */
function fixture(path = "main.tex") {
  const project = createProjectIndex();
  project.update(mainIndex);
  project.update(chapIndex);
  const opened = [];
  const applied = [];
  const context = {
    project: () => project,
    path: () => path,
    open: (target, at) => opened.push([target, at]),
    applyEdits: (edits) => applied.push(edits),
  };
  return { project, context, opened, applied };
}

/** An editor state for `text` with the caret at `head`. */
const stateAt = (text, head) => EditorState.create({ doc: text, selection: { anchor: head } });

test("reference rows carry line numbers on screen and the path alone elsewhere", () => {
  const { context } = fixture();
  const refHead = MAIN.indexOf("\\ref{sec:a}") + 6;
  const locations = [
    { path: "chap.tex", from: CHAP.indexOf("sec:a"), to: CHAP.indexOf("sec:a") + 5 },
    { path: "main.tex", from: MAIN.indexOf("sec:a"), to: MAIN.indexOf("sec:a") + 5 },
    { path: "main.tex", from: refHead - 1, to: refHead + 4 },
  ];
  const rows = referenceRows(stateAt(MAIN, refHead).doc, context, locations);
  assert.deepEqual(rows, [
    { path: "chap.tex", from: locations[0].from, text: "chap.tex" },
    { path: "main.tex", from: locations[1].from, text: m.references_location({ path: "main.tex", line: 1 }) },
    { path: "main.tex", from: locations[2].from, text: m.references_location({ path: "main.tex", line: 2 }) },
  ]);
});

test("reference rows are empty for no locations", () => {
  const { context } = fixture();
  assert.deepEqual(referenceRows(stateAt(MAIN, 0).doc, context, []), []);
});

test("reference rows fall back to the path when the offset is past the end of the file on screen", () => {
  const { context } = fixture();
  const rows = referenceRows(stateAt(MAIN, 0).doc, context, [{ path: "main.tex", from: MAIN.length + 10, to: MAIN.length + 12 }]);
  assert.deepEqual(rows, [{ path: "main.tex", from: MAIN.length + 10, text: "main.tex" }]);
});

test("renameTarget accepts a label and a ref that resolves to a label", () => {
  const { context } = fixture();
  const label = renameTarget(stateAt(MAIN, MAIN.indexOf("sec:a")), context);
  assert.equal(label?.kind, "label");
  assert.equal(label?.name, "sec:a");

  const ref = renameTarget(stateAt(MAIN, MAIN.indexOf("\\ref{sec:a}") + 6), context);
  assert.equal(ref?.kind, "ref");
  assert.equal(ref?.name, "sec:a");
});

test("renameTarget rejects cites, either-refs resolving to cites, and plain text", () => {
  const { context } = fixture();
  assert.equal(renameTarget(stateAt(MAIN, MAIN.indexOf("smith2020")), context), null, "cite");
  assert.equal(renameTarget(stateAt(MAIN, MAIN.indexOf("\\ref{smith2020}") + 6), context), null, "either-ref to a cite");
  assert.equal(renameTarget(stateAt(MAIN, MAIN.indexOf("See") + 1), context), null, "plain text");
});

test("renameTarget accepts an either-ref once a label of that name exists", () => {
  const { project, context } = fixture();
  const head = MAIN.indexOf("\\ref{smith2020}") + 6;
  assert.equal(renameTarget(stateAt(MAIN, head), context), null, "no label named smith2020 yet");
  project.update({
    path: "defs.tex",
    format: "latex",
    length: 20,
    items: [{ kind: "label", name: "smith2020", range: [0, 20], nameRange: [7, 17] }],
  });
  assert.equal(renameTarget(stateAt(MAIN, head), context)?.name, "smith2020", "now a label");
});

test("the references panel follows the index: it recomputes on a refresh and closes when nothing is left", () => {
  const { project, context } = fixture();
  const [field] = references(context);
  const view = {
    state: EditorState.create({ doc: MAIN, selection: { anchor: MAIN.indexOf("sec:a") }, extensions: [field] }),
    dispatch(spec) {
      this.state = this.state.update(spec).state;
    },
  };
  assert.equal(findReferencesCommand(context)(view), true);
  const paths = () => view.state.field(field).rows.map((row) => row.path);
  assert.deepEqual(paths(), ["chap.tex", "main.tex", "main.tex"]);

  // A new file uses the label; the refresh lists it.
  const NEW = "\\ref{sec:a}\n";
  project.update({
    path: "new.tex",
    format: "latex",
    length: NEW.length,
    items: [{ kind: "ref", name: "sec:a", namespace: "label", ...spans(NEW, "\\ref", "sec:a") }],
  });
  view.dispatch({ effects: indexChanged.of(null) });
  assert.deepEqual(paths(), ["chap.tex", "main.tex", "main.tex", "new.tex"]);

  // The label and every reference to it are gone; the refresh closes the panel.
  project.update({ ...mainIndex, items: mainIndex.items.filter((item) => item.name !== "sec:a") });
  project.update({ ...chapIndex, items: [] });
  project.update({ path: "new.tex", format: "latex", length: NEW.length, items: [] });
  view.dispatch({ effects: indexChanged.of(null) });
  assert.equal(view.state.field(field), null);
});

test("the rename panel keeps its constructor while edits move its caret", async () => {
  const { showPanel } = await import("@codemirror/view");
  const { renameCommand, rename: renameExtension } = await import("../../src/lib/document-index-extensions/rename.js");
  const { context } = fixture();
  let state = EditorState.create({ doc: MAIN, extensions: [renameExtension(context)], selection: { anchor: MAIN.indexOf("sec:a") + 1 } });
  const view = {
    get state() {
      return state;
    },
    dispatch(spec) {
      state = state.update(spec).state;
    },
  };
  assert.equal(renameCommand(context)(view), true);
  const [opened] = state.facet(showPanel);
  assert.equal(typeof opened, "function");
  view.dispatch({ changes: { from: 0, insert: "% collaborator line\n" } });
  assert.equal(state.facet(showPanel)[0], opened, "an edit does not rebuild the panel");
});
