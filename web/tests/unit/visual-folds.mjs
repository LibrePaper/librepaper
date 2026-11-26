import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { foldLayer, foldsAllOpen, hasFoldLayer, showAllFolds } from "../../src/lib/visual/folds.js";

// The number of pills drawn: the replaced ranges of the decoration sets the fold layer provides.
function drawnCount(state) {
  return state.facet(EditorView.decorations).reduce((count, set) => (typeof set === "function" ? count : count + set.size), 0);
}

const doc = "preamble text here\n\nsome body text that is long enough\n";
// On the third line, so the default caret at 0 does not reveal it.
const fold = { from: 20, to: 54, kind: "comment" };
const finder = () => [fold];

// One fold is drawn; showing every source draws none; folding again draws it once more.
{
  const state = EditorState.create({ doc, extensions: [foldLayer(finder)] });
  assert.equal(hasFoldLayer(state), true);
  assert.equal(foldsAllOpen(state), false);
  assert.equal(drawnCount(state), 1);

  const open = state.update({ effects: showAllFolds.of(true) }).state;
  assert.equal(drawnCount(open), 0);
  assert.equal(foldsAllOpen(open), true);

  const closed = open.update({ effects: showAllFolds.of(false) }).state;
  assert.equal(drawnCount(closed), 1);
  assert.equal(foldsAllOpen(closed), false);
}

// The flag is carried across a document change.
{
  const open = EditorState.create({ doc, extensions: [foldLayer(finder)] }).update({ effects: showAllFolds.of(true) }).state;
  const edited = open.update({ changes: { from: doc.length, insert: "more\n" } }).state;
  assert.equal(foldsAllOpen(edited), true);
  assert.equal(drawnCount(edited), 0);
}

// Without the fold layer the flag reads as closed and no layer is reported.
{
  const state = EditorState.create({ doc });
  assert.equal(hasFoldLayer(state), false);
  assert.equal(foldsAllOpen(state), false);
}

// The flag survives a reconfiguration that rebuilds the visual layer (a theme change, say).
{
  const { Compartment } = await import("@codemirror/state");
  const visual = new Compartment();
  const open = EditorState.create({ doc, extensions: [visual.of(foldLayer(finder))] }).update({ effects: showAllFolds.of(true) }).state;
  const rebuilt = open.update({ effects: visual.reconfigure(foldLayer(() => [fold])) }).state;
  assert.equal(foldsAllOpen(rebuilt), true);
  assert.equal(drawnCount(rebuilt), 0);
}
