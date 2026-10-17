// What a pending change looks like in the text it changed.
//
// Inserted text is real text and is marked where it sits. Removed text is not
// in the document, so it hangs at the gap it was taken from as a widget. The
// marks follow edits instead of being cleared, and a fresh list replaces them.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { changeMarks, setChangeMarks } from "../../src/lib/change-marks.js";

const field = changeMarks();
const me = { who: "me" };

function found(state) {
  const marks = [];
  state.field(field).between(0, state.doc.length, (from, to, value) => {
    marks.push({
      from, to,
      tag: value.widget?.kind,
      text: value.widget?.text,
      cls: value.spec?.class || value.widget?.className || "",
      title: value.spec?.attributes?.title,
    });
  });
  return marks;
}

function draw(text, payload) {
  const start = EditorState.create({ doc: text, extensions: [field] });
  return start.update({ effects: setChangeMarks.of(payload) }).state;
}

const change = (extra) => ({ id: "c1", who: "peer", author: "Pat", from: 4, to: 7, ...extra });
const gap = (at, fragments) => ({ id: "g1", at, fragments });
const cat = [{ change: "c1", text: "cat" }];

{
  // Inserted text is marked in place, with the author on hover. Another
  // author's change is not the viewer's own.
  const marks = found(draw("The dog sat.", { changes: [change()], gaps: [], me }));
  assert.equal(marks.length, 1);
  assert.deepEqual([marks[0].from, marks[0].to], [4, 7]);
  assert.equal(marks[0].cls, "change-added");
  assert.equal(marks[0].title, "Pat");
}

{
  // The viewer's own change carries the second cue.
  const marks = found(draw("The dog sat.", { changes: [change({ who: "me", author: "Me" })], gaps: [], me }));
  assert.equal(marks[0].cls, "change-added change-mine");
}

{
  // A replacement: the new words are marked, and the old ones stay legible,
  // hung at the gap where they were taken from.
  const marks = found(draw("The dog sat.", { changes: [change()], gaps: [gap(4, cat)], me }));
  const removed = marks.find((mark) => mark.tag === "removed");
  const added = marks.find((mark) => !mark.tag);
  assert.equal(removed.text, "cat", "the words taken out are still readable");
  assert.equal(removed.from, 4, "and they hang where they were taken from");
  assert.deepEqual([added.from, added.to], [4, 7]);
}

{
  // A pure deletion draws only the widget.
  const marks = found(draw("The  sat.", { changes: [change({ from: 4, to: 4 })], gaps: [gap(4, cat)], me }));
  assert.equal(marks.length, 1);
  assert.equal(marks[0].tag, "removed");
}

{
  // Two authors' deletions at one gap: two widgets at the same point, in the
  // order of the fragments.
  const changes = [
    change({ id: "c1", from: 4, to: 4 }),
    change({ id: "c2", who: "me", author: "Me", from: 4, to: 4 }),
  ];
  const fragments = [{ change: "c1", text: "cat" }, { change: "c2", text: "bird" }];
  const marks = found(draw("The  sat.", { changes, gaps: [gap(4, fragments)], me }));
  const widgets = marks.filter((mark) => mark.tag === "removed");
  assert.deepEqual(widgets.map((mark) => mark.text), ["cat", "bird"], "the fragments are not in document order");
  assert.deepEqual(widgets.map((mark) => mark.from), [4, 4]);
  assert.equal(widgets[0].cls, "", "another author's deletion was drawn as the viewer's own");
  assert.equal(widgets[1].cls, "change-mine", "the viewer's own deletion was not marked as theirs");
}

{
  // The selected change is outlined, on both the mark and the widget.
  const marks = found(draw("The dog sat.", { changes: [change()], gaps: [gap(4, cat)], me, selected: "c1" }));
  assert.ok(marks.every((mark) => mark.cls.includes("change-selected")), JSON.stringify(marks));
  const other = found(draw("The dog sat.", { changes: [change()], gaps: [gap(4, cat)], me, selected: "other" }));
  assert.ok(other.every((mark) => !mark.cls.includes("change-selected")));
}

{
  // A change that does not fit the text is not drawn rather than clamped.
  assert.deepEqual(found(draw("short", { changes: [change({ from: 40, to: 43 })], gaps: [], me })), []);
}

{
  // Typing before a mark moves it. The marks are mapped, not cleared, until
  // the caller sends a fresh list.
  const drawn = draw("The dog sat.", { changes: [change()], gaps: [gap(4, cat)], me });
  const edited = drawn.update({ changes: { from: 0, insert: "Yesterday. " } }).state;
  const marks = found(edited);
  assert.equal(marks.length, 2, "an edit elsewhere does not drop the marks");
  const added = marks.find((mark) => !mark.tag);
  assert.deepEqual([added.from, added.to], [15, 18]);
  assert.equal(marks.find((mark) => mark.tag === "removed").from, 15);
  const redrawn = edited.update({ effects: setChangeMarks.of({ changes: [], gaps: [], me }) }).state;
  assert.deepEqual(found(redrawn), [], "a fresh list replaces the mapped marks");
}

console.log("change-marks: pending changes are drawn in the text, mapped through edits, and replaced on request");
