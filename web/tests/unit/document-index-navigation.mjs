// Go to definition: the command opens the first definition of the reference at the
// caret, and does nothing where there is none. Run against a real EditorState and
// a fake view and context, since Node has no DOM for an EditorView.
import assert from "node:assert/strict";
import test from "node:test";
import { EditorState } from "@codemirror/state";
import { createLiveIndex } from "../../src/lib/document-index/live.js";
import { goToDefinition } from "../../src/lib/document-index-extensions/navigation.js";

const main = "See \\ref{fig:a} and \\ref{fig:missing}.\n";
const intro = "\\section{Intro}\n\\label{fig:a}\n";

/**
 * @param {string} text
 * @param {number} pos
 * @returns {{ view: { state: EditorState }, opened: Array<[string, number]>, context: object }}
 */
function setup(text, pos) {
  const live = createLiveIndex();
  live.refresh({
    texts: { "main.tex": main, "intro.tex": intro },
    files: ["main.tex", "intro.tex"],
    bibliography: null,
  });
  /** @type {Array<[string, number]>} */
  const opened = [];
  const context = {
    project: () => live.project,
    path: () => "main.tex",
    open: (path, at) => opened.push([path, at]),
    applyEdits: () => {},
  };
  const view = { state: EditorState.create({ doc: text, selection: { anchor: pos } }) };
  return { view, opened, context };
}

test("goToDefinition opens the label a reference points to in another file", () => {
  const pos = main.indexOf("fig:a") + 2;
  const { view, opened, context } = setup(main, pos);

  assert.equal(goToDefinition(view, context), true);
  assert.deepEqual(opened, [["intro.tex", intro.indexOf("fig:a")]]);
});

test("goToDefinition does nothing and returns false where there is no definition", () => {
  const pos = main.indexOf("fig:missing") + 2;
  const { view, opened, context } = setup(main, pos);

  assert.equal(goToDefinition(view, context), false);
  assert.deepEqual(opened, []);
});

test("goToDefinition does nothing on plain text", () => {
  const pos = main.indexOf("See") + 1;
  const { view, opened, context } = setup(main, pos);

  assert.equal(goToDefinition(view, context), false);
  assert.deepEqual(opened, []);
});
