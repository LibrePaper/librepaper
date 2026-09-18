import assert from "node:assert/strict";
import { LoroDoc, LoroText } from "loro-crdt";
import { draftMarksOf, hunksOfProposal } from "../../src/lib/proposals.js";

// An author with track changes on is reading their own branch, so what they
// need drawn is the branch's changes against the text in front of them -- the
// tip -- and not against the base, which is what a reviewer of the same branch
// reads. These assert that the two bases really are different and that this
// one lands on the words the author can see.

const OWNER = 1n;
const AUTHOR = 2n;

function draft(text, edit) {
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  const files = room.getMap("files");
  const file = files.setContainer("f1", new LoroText());
  file.insert(0, text);
  room.commit();
  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);
  edit(branch.getMap("files").get("f1"));
  branch.commit();
  return { room, base, branch };
}

// A deletion is gone from the text the author sees, so it is hung at the point
// it was taken from and carries the words with it.
{
  const { room, base, branch } = draft("The cat sat on the mat.", (text) => text.delete(4, 4));
  const marks = draftMarksOf(room, base, branch);
  assert.equal(branch.getMap("files").get("f1").toString(), "The sat on the mat.");
  assert.deepEqual(marks, [{ kind: "del", file: "f1", at: 4, text: "cat ", hunk: 0 }]);
}

// An insertion is really there, so it is a range of the text on screen.
{
  const { room, base, branch } = draft("The cat sat.", (text) => text.insert(4, "big "));
  const marks = draftMarksOf(room, base, branch);
  assert.deepEqual(marks, [{ kind: "ins", file: "f1", at: 4, length: 4, hunk: 0 }]);
  const shown = branch.getMap("files").get("f1").toString();
  assert.equal(shown.slice(4, 8), "big ");
}

// A replacement is both, and the insertion is offset by the deletion having
// left the text -- which is the whole reason the reviewer's basis cannot be
// reused here.
{
  const { room, base, branch } = draft("The cat sat.", (text) => {
    text.delete(4, 3);
    text.insert(4, "tabby");
  });
  const marks = draftMarksOf(room, base, branch);
  const shown = branch.getMap("files").get("f1").toString();
  assert.equal(shown, "The tabby sat.");
  const inserted = marks.find((mark) => mark.kind === "ins");
  const removed = marks.find((mark) => mark.kind === "del");
  assert.equal(shown.slice(inserted.at, inserted.at + inserted.length), "tabby");
  assert.equal(removed.text, "cat");
  // The same branch read as a reviewer reads it: an offset into the base, where
  // "tabby" is not. Drawing that over the tip is what this replaces.
  const tip = branch.frontiers();
  const bytes = branch.export({ mode: "update", from: room.oplogVersion() });
  const hunks = hunksOfProposal(room, { base, tip, bytes });
  assert.equal(hunks.length, 1);
  assert.equal(hunks[0].deleted, 3);
  assert.equal("The cat sat.".slice(hunks[0].start, hunks[0].start + hunks[0].deleted), "cat");
}

// Two edits far apart are two hunks, and every mark says which one it belongs
// to so a mark can be pointed at from the Changes panel.
{
  const { room, base, branch } = draft("The cat sat. The dog ran.", (text) => {
    text.delete(17, 3);
    text.insert(17, "fox");
    text.delete(4, 3);
    text.insert(4, "owl");
  });
  const marks = draftMarksOf(room, base, branch);
  assert.deepEqual([...new Set(marks.map((mark) => mark.hunk))], [0, 1]);
  const shown = branch.getMap("files").get("f1").toString();
  for (const mark of marks.filter((one) => one.kind === "ins")) {
    assert.ok(["owl", "fox"].includes(shown.slice(mark.at, mark.at + mark.length)));
  }
}

// Nothing typed yet is nothing to draw, and an unreachable base draws nothing
// rather than throwing at the author.
{
  const { room, base, branch } = draft("Untouched.", () => {});
  assert.deepEqual(draftMarksOf(room, base, branch), []);
  assert.deepEqual(draftMarksOf(null, base, branch), []);
  assert.deepEqual(draftMarksOf(room, base, null), []);
}

console.log("proposal-draft-marks: ok");
