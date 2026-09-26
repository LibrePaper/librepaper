// What a proposal looks like in the text it would change.
//
// The decisions worth pinning are about restraint: a hunk that no longer fits
// the document is not drawn at all, and a hunk belonging to another file is not
// drawn here. Both are the same rule -- a suggestion drawn in the wrong place is
// worse than one not drawn, because a reviewer can agree to it.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { proposalMarks, setProposalMarks } from "../../src/lib/proposal-marks.js";

const field = proposalMarks();

function draw(text, payload) {
  const start = EditorState.create({ doc: text, extensions: [field] });
  const next = start.update({ effects: setProposalMarks.of(payload) }).state;
  const found = [];
  next.field(field).between(0, next.doc.length, (from, to, value) => {
    found.push({ from, to, tag: value.widget?.kind, text: value.widget?.text, cls: value.spec?.class || value.widget?.className || "" });
  });
  return found;
}

const proposal = (hunks, id = "p1") => ({ id, author: "Ada", hunks });

{
  // A replacement: the old words stay legible, struck through, and the new
  // ones sit beside them.
  const marks = draw("The cat sat.", {
    proposals: [proposal([{ index: 0, start: 4, deleted: 3, inserted: "tabby", file: "f1" }])],
    showing: "f1",
  });
  assert.equal(marks.length, 2, `expected a removal and an addition, got ${JSON.stringify(marks)}`);
  const removed = marks.find((mark) => mark.tag === "removed");
  const added = marks.find((mark) => mark.tag === "added");
  assert.equal(removed.text, "cat", "the words being taken out are still readable");
  assert.equal(added.text, "tabby", "and what would replace them is shown");
  assert.equal(removed.from, 4);
  assert.equal(added.from, 7, "the addition sits where the removal ends");
}

{
  // An insertion with nothing removed draws only the addition.
  const marks = draw("The sat.", {
    proposals: [proposal([{ index: 0, start: 4, deleted: 0, inserted: "cat ", file: "f1" }])],
    showing: "f1",
  });
  assert.equal(marks.length, 1);
  assert.equal(marks[0].tag, "added");
  assert.equal(marks[0].text, "cat ");
}

{
  // A hunk reaching past the end of the document is a hunk about a different
  // text than the one on screen. It is not drawn.
  const marks = draw("short", {
    proposals: [proposal([{ index: 0, start: 40, deleted: 5, inserted: "x", file: "f1" }])],
    showing: "f1",
  });
  assert.deepEqual(marks, [], "a hunk that does not fit is left undrawn rather than clamped");
}

{
  // A hunk in another file is not drawn in this one, however well its offsets
  // happen to fit.
  const marks = draw("The cat sat.", {
    proposals: [proposal([{ index: 0, start: 4, deleted: 3, inserted: "tabby", file: "other" }])],
    showing: "f1",
  });
  assert.deepEqual(marks, [], "offsets that fit by coincidence are not an invitation");
}

{
  // Two people proposing different words for the same passage both appear.
  // Neither is chosen here; that is §5.3's question, and this is only drawing.
  const marks = draw("The cat sat.", {
    proposals: [
      proposal([{ index: 0, start: 4, deleted: 3, inserted: "tabby", file: "f1" }], "p1"),
      proposal([{ index: 0, start: 4, deleted: 3, inserted: "lion", file: "f1" }], "p2"),
    ],
    showing: "f1",
  });
  const added = marks.filter((mark) => mark.tag === "added").map((mark) => mark.text).sort();
  assert.deepEqual(added, ["lion", "tabby"], "both proposals are visible, not just the first");
}

{
  // Editing the text moves every offset these were placed against, so they are
  // dropped rather than mapped: a hunk is a claim about the proposal's base,
  // and mapping it through an edit turns it into a claim about something else.
  const start = EditorState.create({ doc: "The cat sat.", extensions: [field] });
  const drawn = start.update({
    effects: setProposalMarks.of({
      proposals: [proposal([{ index: 0, start: 4, deleted: 3, inserted: "tabby", file: "f1" }])],
      showing: "f1",
    }),
  }).state;
  assert.equal(drawn.field(field).size, 2);
  const edited = drawn.update({ changes: { from: 0, insert: "Yesterday. " } }).state;
  assert.equal(edited.field(field).size, 0, "typing clears the marks until they are recomputed");
}

{
  // The author's own draft, drawn in the other basis. The editor is bound to
  // the branch while track changes is on, so an insertion is text that is
  // really on screen -- marked, not shown a second time -- and a deletion is
  // words that are not, hung at the point they were taken from.
  const marks = draw("The owl sat.", {
    showing: "f1",
    draft: [
      { kind: "del", file: "f1", at: 4, text: "cat ", hunk: 0 },
      { kind: "ins", file: "f1", at: 4, length: 4, hunk: 0 },
    ],
  });
  const removed = marks.find((mark) => mark.tag === "removed");
  const added = marks.find((mark) => !mark.tag);
  assert.equal(removed.text, "cat ", "the words taken out are readable, though they are gone from the text");
  assert.equal(removed.from, 4, "and they hang where they were");
  assert.equal(added.from, 4);
  assert.equal(added.to, 8, "the insertion covers the text the author typed");
  assert.ok(removed.cls.includes("proposal-mine") && added.cls.includes("proposal-mine"),
    "the author's own marks are the coloured ones");
}

{
  // A draft mark belonging to another file is left alone, the same rule the
  // review marks follow -- and one reaching past the end of this text is not
  // drawn rather than clamped.
  assert.deepEqual(draw("The owl sat.", {
    showing: "f1",
    draft: [{ kind: "ins", file: "other", at: 4, length: 3, hunk: 0 }],
  }), []);
  assert.deepEqual(draw("short", {
    showing: "f1",
    draft: [{ kind: "ins", file: "f1", at: 2, length: 40, hunk: 0 }],
  }), []);
}

console.log("proposal-marks: a proposal is drawn where it fits, and nowhere else");
