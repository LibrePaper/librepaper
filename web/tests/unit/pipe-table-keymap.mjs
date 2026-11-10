import assert from "node:assert/strict";
import test from "node:test";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { org } from "../../src/lib/org-language/index.js";
import { pandocExtensions } from "../../src/lib/pandoc-markdown.js";
import { pipeTableAt, pipeTableKeymap } from "../../src/lib/pipe-table-keymap.js";
import { loadOrgSyntaxForTests } from "../helpers/org-syntax.mjs";

await loadOrgSyntaxForTests();

// A view stand-in: the keys read the state and dispatch changes, and nothing else.
function editor(doc, anchor, extensions) {
  const view = {
    state: EditorState.create({ doc, selection: { anchor }, extensions }),
    dispatch(spec) {
      view.state = view.state.update(spec).state;
    }
  };
  return view;
}

function press(bindings, key, view) {
  return bindings.find(binding => binding.key === key).run(view);
}

const orgKeys = pipeTableKeymap("org", pipeTableAt);
const markdownKeys = pipeTableKeymap("markdown", pipeTableAt);
const orgDoc = "| Name | Qty |\n|---+---|\n| apple | 3 |";
const markdownDoc = "| Name | Qty |\n| :--- | ---: |\n| apple | 3 |";

test("Tab realigns the table and moves to the next cell", () => {
  const view = editor(orgDoc, orgDoc.indexOf("Name") + 2, org());
  assert.equal(press(orgKeys, "Tab", view), true);
  const expected = "| Name  | Qty |\n|-------+-----|\n| apple | 3   |";
  assert.equal(view.state.doc.toString(), expected);
  assert.equal(view.state.selection.main.head, expected.indexOf("Qty"));
});

test("Shift-Tab moves to the previous cell", () => {
  const view = editor(orgDoc, orgDoc.indexOf("Qty") + 1, org());
  assert.equal(press(orgKeys, "Shift-Tab", view), true);
  assert.equal(view.state.selection.main.head, view.state.doc.toString().indexOf("Name"));
});

test("Tab past the last cell adds a row and moves into its first cell", () => {
  const doc = "| a | b |";
  const view = editor(doc, doc.indexOf("b"), org());
  assert.equal(press(orgKeys, "Tab", view), true);
  const expected = "| a | b |\n|   |   |";
  assert.equal(view.state.doc.toString(), expected);
  // The first cell of the new row is empty: its text starts before the closing pipe.
  assert.equal(view.state.selection.main.head, expected.indexOf("\n") + 1 + 4);
});

test("Enter moves down the same column, skipping the rule", () => {
  const doc = "| a | b |\n|---+---|\n| c | d |";
  const view = editor(doc, doc.indexOf("b"), org());
  assert.equal(press(orgKeys, "Enter", view), true);
  assert.equal(view.state.doc.toString(), doc);
  assert.equal(view.state.selection.main.head, doc.lastIndexOf("d"));
});

test("Enter on the last row adds a row at the end", () => {
  const doc = "| a | b |\n|---+---|\n| c | d |";
  const view = editor(doc, doc.indexOf("d"), org());
  assert.equal(press(orgKeys, "Enter", view), true);
  const expected = "| a | b |\n|---+---|\n| c | d |\n|   |   |";
  assert.equal(view.state.doc.toString(), expected);
  // Enter keeps the column: the cursor is in the second cell, before its closing pipe.
  assert.equal(view.state.selection.main.head, expected.length - 1);
});

test("Tab and Enter fall through outside a table", () => {
  const doc = "plain text\n\n| a | b |";
  const view = editor(doc, 2, org());
  assert.equal(press(orgKeys, "Tab", view), false);
  assert.equal(press(orgKeys, "Shift-Tab", view), false);
  assert.equal(press(orgKeys, "Enter", view), false);
  assert.equal(view.state.doc.toString(), doc);
});

test("Org: a |- line at the end expands into a rule matching the table above", () => {
  const doc = "| a | bb |\n|-";
  const view = editor(doc, doc.length, org());
  assert.equal(press(orgKeys, "Tab", view), true);
  const expected = "| a | bb |\n|---+----|";
  assert.equal(view.state.doc.toString(), expected);
  assert.equal(view.state.selection.main.head, expected.length);
});

test("Markdown: Tab realigns with the separator's alignment kept", () => {
  const view = editor(markdownDoc, markdownDoc.indexOf("apple") + 2, markdown({ base: markdownLanguage, extensions: pandocExtensions }));
  assert.equal(press(markdownKeys, "Tab", view), true);
  const expected = "| Name  | Qty |\n|:------|----:|\n| apple |   3 |";
  assert.equal(view.state.doc.toString(), expected);
  assert.equal(view.state.selection.main.head, expected.lastIndexOf("3"));
});

test("Enter on an empty last row leaves the table", () => {
  const doc = "| a | b |\n|   |   |";
  const view = editor(doc, doc.length - 3, org());
  assert.equal(press(orgKeys, "Enter", view), true);
  assert.equal(view.state.doc.toString(), "| a | b |\n");
  assert.equal(view.state.selection.main.head, view.state.doc.length);
});
