// Comment marks in the source: where they sit, how they follow edits, and
// which entries are dropped or lose their colour.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { commentMarks, setCommentMarks } from "../../src/lib/source-comment-marks.js";

const marks = commentMarks({ onselect() {} });
const [field] = marks;

function found(state) {
  const marks = [];
  state.field(field).between(0, state.doc.length, (from, to, value) => {
    marks.push({
      from,
      to,
      cls: value.spec?.class || "",
      id: value.spec?.attributes?.["data-comment-id"],
      style: value.spec?.attributes?.style,
    });
  });
  return marks;
}

function draw(text, list) {
  const start = EditorState.create({ doc: text, extensions: marks });
  return start.update({ effects: setCommentMarks.of(list) }).state;
}

const entry = (extra) => ({ id: "c1", from: 4, to: 7, highlight: false, color: null, selected: false, ...extra });

{
  // A comment is drawn over its passage, carrying its id for a click.
  const marks = found(draw("The dog sat.", [entry()]));
  assert.equal(marks.length, 1);
  assert.deepEqual([marks[0].from, marks[0].to], [4, 7]);
  assert.equal(marks[0].cls, "cm-comment-mark");
  assert.equal(marks[0].id, "c1");
  assert.equal(marks[0].style, undefined, "a plain comment has no colour");
}

{
  // A selected highlight carries the highlight and selected classes.
  const marks = found(draw("The dog sat.", [entry({ highlight: true, selected: true })]));
  assert.equal(marks[0].cls, "cm-comment-mark cm-comment-mark-highlight cm-comment-mark-selected");
}

{
  // A valid hex colour on a highlight becomes the custom property.
  const marks = found(draw("The dog sat.", [entry({ highlight: true, color: "#AABBCC" })]));
  assert.equal(marks[0].style, "--comment-mark-color: #AABBCC");
}

{
  // A colour that is not a six-digit hex is never written into the style.
  const marks = found(draw("The dog sat.", [entry({ highlight: true, color: "red; background:url(x)" })]));
  assert.equal(marks[0].style, undefined);
  assert.equal(marks[0].cls, "cm-comment-mark cm-comment-mark-highlight");
}

{
  // A colour on a plain comment is not used either: only highlights are coloured.
  const marks = found(draw("The dog sat.", [entry({ highlight: false, color: "#aabbcc" })]));
  assert.equal(marks[0].style, undefined);
}

{
  // Marks follow an insertion before them.
  const start = draw("The dog sat.", [entry()]);
  const after = start.update({ changes: { from: 0, insert: "Oh, " } }).state;
  const marks = found(after);
  assert.deepEqual([marks[0].from, marks[0].to], [8, 11]);
}

{
  // Entries with nothing to cover are dropped, and one running past the end is clamped.
  const marks = found(draw("The dog sat.", [entry({ from: 5, to: 5 }), entry({ id: "c2", from: 6, to: 2 }), entry({ id: "c3", from: 9, to: 99 })]));
  assert.equal(marks.length, 1);
  assert.equal(marks[0].id, "c3");
  assert.deepEqual([marks[0].from, marks[0].to], [9, 12]);
}

{
  // A fresh list replaces the marks drawn before it.
  const first = draw("The dog sat.", [entry()]);
  const second = first.update({ effects: setCommentMarks.of([]) }).state;
  assert.equal(found(second).length, 0);
}
