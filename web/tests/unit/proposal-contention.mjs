// Two answers to the same question, and reading a subset as finished prose.
//
// Both are SPEC-loro.md §5.3, and both are computed in the browser from what
// `proposal-list` already carries. What is worth pinning is where the line
// falls: which pairs of rows are a choice between them and which are two
// things that can both happen, and that a preview never touches the document
// it previews.
import assert from "node:assert/strict";
import { LoroDoc, LoroText } from "loro-crdt";
import { markContention, previewTexts } from "../../src/lib/proposals.js";

function row(id, proposal, file, position, before, extra = {}) {
  return {
    id,
    proposal,
    file_id: file,
    position,
    before,
    after: "",
    stale: "",
    ...extra,
  };
}

function groupsOf(rows) {
  const byGroup = new Map();
  for (const item of rows) {
    if (!item.contested) continue;
    if (!byGroup.has(item.contested)) byGroup.set(item.contested, []);
    byGroup.get(item.contested).push(item.id);
  }
  return [...byGroup.values()].map((ids) => ids.sort());
}

// Two people rewriting one sentence. This is the case the grouping exists for.
{
  const marked = markContention([
    row("a#0", "a", "paper", 4, "cat"),
    row("b#0", "b", "paper", 4, "cat sat"),
  ]);
  assert.deepEqual(groupsOf(marked), [["a#0", "b#0"]]);
}

// The same proposal twice is not contention with itself. An author who
// changed two things in one file made one offer, not a choice.
{
  const marked = markContention([
    row("a#0", "a", "paper", 0, "The cat"),
    row("a#1", "a", "paper", 0, "The cat sat"),
  ]);
  assert.deepEqual(groupsOf(marked), []);
  assert.equal(marked.every((item) => item.contested === ""), true);
}

// Different files never contend, however the offsets fall.
{
  const marked = markContention([
    row("a#0", "a", "paper", 4, "cat"),
    row("b#0", "b", "notes", 4, "cat"),
  ]);
  assert.deepEqual(groupsOf(marked), []);
}

// Adjacent edits are not a choice. One replaces "The", the other replaces
// "cat" immediately after it; both can happen and grouping them would ask the
// reviewer to give one up for no reason.
{
  const marked = markContention([
    row("a#0", "a", "paper", 0, "The"),
    row("b#0", "b", "paper", 3, " cat"),
  ]);
  assert.deepEqual(groupsOf(marked), []);
}

// Two insertions in the same gap are two additions, not rival ones: each has
// nothing to remove, so neither is an answer to the other.
{
  const marked = markContention([
    row("a#0", "a", "paper", 7, ""),
    row("b#0", "b", "paper", 7, ""),
  ]);
  assert.deepEqual(groupsOf(marked), []);
}

// Three proposals over one sentence are one group, including the pair that do
// not touch each other directly -- a and c both contend with b, so all three
// are answers to the same question.
{
  const marked = markContention([
    row("a#0", "a", "paper", 0, "The cat"),
    row("b#0", "b", "paper", 4, "cat sat"),
    row("c#0", "c", "paper", 8, "sat on"),
  ]);
  assert.deepEqual(groupsOf(marked), [["a#0", "b#0", "c#0"]]);
}

// A stale row is not grouped. Its extent describes text that is not there, so
// it cannot be said to overlap anything -- and it cannot be answered either.
{
  const marked = markContention([
    row("a#0", "a", "paper", 4, "cat", { stale: "The text has changed." }),
    row("b#0", "b", "paper", 4, "cat sat"),
  ]);
  assert.deepEqual(groupsOf(marked), []);
}

// Rows keep their order and their fields; grouping annotates, it does not
// rearrange. The queue's order is somebody's reading order.
{
  const input = [
    row("b#0", "b", "paper", 4, "cat sat"),
    row("a#0", "a", "paper", 4, "cat"),
  ];
  const marked = markContention(input);
  assert.deepEqual(
    marked.map((item) => item.id),
    ["b#0", "a#0"],
  );
  assert.equal(marked[0].before, "cat sat");
  assert.equal(input[0].contested, undefined, "the input rows are not mutated");
}

console.log("proposal-contention: rival edits group, adjacent and stale ones do not");

// --- reading a subset -------------------------------------------------------

{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const files = room.getMap("files");
  const body = files.setContainer("f1", new LoroText());
  body.insert(0, "one. two. three.");
  room.commit();

  const alice = room.fork();
  alice.setPeerId(2n);
  alice.getMap("files").get("f1").insert(0, "ONE! ");
  alice.commit();

  const bob = room.fork();
  bob.setPeerId(3n);
  const bobBody = bob.getMap("files").get("f1");
  bobBody.insert(bobBody.length, " THREE!");
  bob.commit();

  const proposals = [
    { id: "alice", bytes: alice.export({ mode: "update" }) },
    { id: "bob", bytes: bob.export({ mode: "update" }) },
  ];

  const both = previewTexts(room, proposals, ["alice", "bob"]);
  assert.ok(both, "a preview of two proposals is readable");
  const merged = both.get("f1");
  assert.match(merged, /ONE!/, "Alice's change is in the reading");
  assert.match(merged, /THREE!/, "and Bob's is too");

  const onlyAlice = previewTexts(room, proposals, ["alice"]);
  assert.match(onlyAlice.get("f1"), /ONE!/);
  assert.doesNotMatch(
    onlyAlice.get("f1"),
    /THREE!/,
    "a subset is a subset: Bob's change is not in a reading that did not ask for it",
  );

  // The point of forking rather than applying: the room is untouched by having
  // been read speculatively.
  assert.equal(
    room.getMap("files").get("f1").toString(),
    "one. two. three.",
    "previewing does not move the document",
  );

  assert.equal(previewTexts(room, proposals, []), null, "nothing chosen is nothing to read");
  assert.equal(previewTexts(null, proposals, ["alice"]), null);
}

console.log("proposal-contention: a subset of proposals reads as prose, and the room does not move");
