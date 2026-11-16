// Go to definition: the command opens the first definition of the reference at the
// caret, and does nothing where there is none. Run against a real EditorState and
// a fake view and context, since Node has no DOM for an EditorView.
import assert from "node:assert/strict";
import test from "node:test";
import { EditorState } from "@codemirror/state";
import { createLiveIndex } from "../../src/lib/document-index/live.js";
import { createProjectIndex } from "../../src/lib/document-index/project.js";
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

/**
 * Go-to-definition over a hand-written project, for items the live index does not
 * produce yet: a file link and a footnote.
 * @param {string} path the file the caret is in
 * @param {string} text its text
 * @param {number} pos the caret
 * @param {Array<[string, string, Array<object>]>} files [path, format, items] for each file
 * @returns {{ view: { state: EditorState }, opened: Array<[string, number]>, context: object }}
 */
function setupProject(path, text, pos, files) {
  const project = createProjectIndex();
  project.setFiles(files.map(([filePath]) => filePath));
  for (const [filePath, format, items, fileText] of files) {
    project.update({ path: filePath, format, length: fileText.length, items });
  }
  /** @type {Array<[string, number]>} */
  const opened = [];
  const context = {
    project: () => project,
    path: () => path,
    open: (openPath, at) => opened.push([openPath, at]),
    applyEdits: () => {},
  };
  const view = { state: EditorState.create({ doc: text, selection: { anchor: pos } }) };
  return { view, opened, context };
}

test("goToDefinition opens the file a file link names, and does nothing for a URL", () => {
  const text = "[guide](sections/rendering.md#top) and [site](https://e.org)\n";
  const file = "sections/rendering.md";
  const fileName = text.indexOf(file);
  const url = text.indexOf("https://e.org");
  const items = [
    { kind: "link", range: [fileName - 1, fileName + file.length + 4], nameRange: [fileName, fileName + file.length], name: file, attrs: { file: "true", fragment: "top" } },
    { kind: "link", range: [url, url + 13], nameRange: [url, url + 13], name: "https://e.org", attrs: { external: "true" } },
  ];
  const linkedFile = "# Rendering\n";
  const main = setupProject("main.md", text, fileName + 2, [
    ["main.md", "markdown", items, text],
    ["sections/rendering.md", "markdown", [], linkedFile],
  ]);
  assert.equal(goToDefinition(main.view, main.context), true);
  assert.deepEqual(main.opened, [["sections/rendering.md", 0]]);

  const external = setupProject("main.md", text, url + 2, [
    ["main.md", "markdown", items, text],
    ["sections/rendering.md", "markdown", [], linkedFile],
  ]);
  assert.equal(goToDefinition(external.view, external.context), false);
  assert.deepEqual(external.opened, []);
});

test("goToDefinition opens the footnote a footnote reference names, not a label of the same name", () => {
  const text = "A note[^n] and {#n}.\n\n[^n]: the body\n";
  const refName = text.indexOf("n]");
  const labelName = text.indexOf("#n") + 1;
  const defName = text.indexOf("[^n]:") + 2;
  const items = [
    { kind: "ref", range: [text.indexOf("[^n]"), text.indexOf("[^n]") + 4], nameRange: [refName, refName + 1], name: "n", namespace: "footnote" },
    { kind: "label", range: [text.indexOf("{#n}"), text.indexOf("{#n}") + 4], nameRange: [labelName, labelName + 1], name: "n", namespace: "label" },
    { kind: "label", range: [text.indexOf("[^n]:"), text.length - 1], nameRange: [defName, defName + 1], name: "n", namespace: "footnote", attrs: { target: "footnote" } },
  ];
  const note = setupProject("notes.md", text, refName, [["notes.md", "markdown", items, text]]);
  assert.equal(goToDefinition(note.view, note.context), true);
  assert.deepEqual(note.opened, [["notes.md", defName]]);
});
