// The bar over a selection in the code pane.
//
// It is a tooltip over a range, and only over a range: a caret has nothing to
// comment on. It is also only there while the caller says the reader may
// annotate, and the caller can change that without rebuilding the editor.
import assert from "node:assert/strict";
import { EditorSelection, EditorState } from "@codemirror/state";
import { showTooltip } from "@codemirror/view";
import { sourceCommentBar } from "../../src/lib/source-comment-bar.js";

const verbs = () => [{ id: "comment", label: "Comment", title: "Comment" }];

// The tooltips the editor would show: the facet holds one entry per field,
// null when the field has nothing to show.
function shown(state) {
  return state.facet(showTooltip).filter(Boolean);
}

function stateWith(selection, enabled) {
  return EditorState.create({
    doc: "The dog sat.",
    selection,
    extensions: [sourceCommentBar({ enabled, verbs, onverb() {} })],
  });
}

{
  // A caret is not a selection, so there is nothing to offer a verb over.
  const state = stateWith(EditorSelection.cursor(4), () => true);
  assert.equal(shown(state).length, 0, "no bar over a caret");
}

{
  // A range, with the reader allowed to annotate, gets the bar at its head.
  const state = stateWith(EditorSelection.range(4, 7), () => true);
  const tips = shown(state);
  assert.equal(tips.length, 1, "a bar over a range");
  assert.equal(tips[0].pos, 7, "anchored at the head of the selection");
  assert.equal(tips[0].above, false, "below a selection that runs forwards");
  assert.equal(tips[0].strictSide, false);
  assert.equal(tips[0].arrow, false);
}

{
  // A backwards selection's head is at its start, so the bar goes above.
  const state = stateWith(EditorSelection.range(7, 4), () => true);
  const tips = shown(state);
  assert.equal(tips[0].pos, 4);
  assert.equal(tips[0].above, true, "above a selection that runs backwards");
}

{
  // The reader may not annotate: no bar, even over a range.
  const state = stateWith(EditorSelection.range(4, 7), () => false);
  assert.equal(shown(state).length, 0, "no bar when annotating is off");
}

{
  // Changing the caller's answer takes effect on the next transaction, not
  // on a reconfigure: the same state, a selection moved, and the answer
  // turned off.
  let allowed = false;
  const state = stateWith(EditorSelection.range(4, 7), () => allowed);
  assert.equal(shown(state).length, 0);
  allowed = true;
  const next = state.update({ selection: EditorSelection.range(0, 3) }).state;
  assert.equal(shown(next).length, 1, "shown after the next transaction once allowed");
  allowed = false;
  const off = next.update({ selection: EditorSelection.range(0, 2) }).state;
  assert.equal(shown(off).length, 0, "hidden on the next transaction once disallowed");
}
