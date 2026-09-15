import assert from "node:assert/strict";
import { LoroDoc, encodeFrontiers, decodeFrontiers } from "loro-crdt";
import { createProposals } from "../../src/lib/proposals.js";

// This test asserts that the browser groups hunks exactly as the server does.
// The server's grouping is in crates/librepaper/src/document/hunks.rs.
// A mismatch means "decline the second hunk" reverts something else.

const AUTHOR = 2n;
const REVIEWER = 3n;
const OWNER = 1n;

// Build a room document and a proposal branch, returning the base and branch
// bytes so hunksOfLocal can rebuild them.
function createTestScenario() {
  const room = new LoroDoc();
  room.setPeerId(OWNER);

  // Single file with some text
  room.getText("main.md").insert(0, "The cat sat. The dog ran.");
  room.commit();

  const base = room.frontiers();
  // What the room has before the branch adds anything, so the export below is
  // the branch's own operations and not the whole document.
  const atFork = room.oplogVersion();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  // Make two edits: replace "cat" with "tabby" and "dog ran" with "dog sprinted"
  // Later edit first, so the earlier one's offsets still hold. "cat" sits at
  // 4..7 and "dog ran" at 17..24, ten retained characters apart -- more than
  // SAME_DECISION_WITHIN, so these stay two decisions.
  const text = branch.getText("main.md");
  text.delete(17, 7);
  text.insert(17, "dog sprinted");
  text.delete(4, 3);
  text.insert(4, "tabby");
  branch.commit();

  const tip = branch.frontiers();

  return { room, branch, base, tip, bytes: branch.export({ mode: "update", from: atFork }) };
}

// Test: A delete and the insert replacing it are ONE hunk
{
  const { room, branch, base, tip, bytes } = createTestScenario();
  const proposal = {
    id: "test-1",
    base,
    tip,
    branch,
  };

  // Simulate what proposals.js does
  const proposalData = {
    base,
    tip,
    bytes
  };

  // Create a fresh proposals instance to test hunksOf
  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Two edits but grouped into one hunk each (one for "cat"→"tabby",
  // one for "dog ran"→"dog sprinted")
  // Actually, they're separated by more than 8 chars so should be two hunks
  assert.equal(hunks.length, 2, "two replacements separated by > 8 chars = two hunks");
  assert.equal(hunks[0].deleted, 3, "first hunk deletes 'cat'");
  assert.equal(hunks[0].inserted, "tabby", "first hunk inserts 'tabby'");
  assert.equal(hunks[1].deleted, 7, "second hunk deletes 'dog ran'");
  assert.equal(hunks[1].inserted, "dog sprinted", "second hunk inserts 'dog sprinted'");
}

// Test: Two changes separated by 8 or fewer retained units are ONE hunk
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  room.getText("main.md").insert(0, "The catX dog ran.");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  const text = branch.getText("main.md");
  // Replace "cat" then "dog ran" with only one char between: "X"
  text.delete(4, 3); // "cat"
  text.insert(4, "tabby");
  text.delete(13, 7); // "dog ran"
  text.insert(13, "bird");
  branch.commit();

  const tip = branch.frontiers();

  const proposalData = {
    base,
    tip,
    bytes
  };

  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Two changes separated by 1 char should be ONE hunk (≤ 8 = same decision)
  assert.equal(hunks.length, 1, "changes separated by 1 char = one hunk");
}

// Test: Two changes separated by 9 or more are TWO hunks
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  room.getText("main.md").insert(0, "The catXXXXXXXX dog ran.");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  const text = branch.getText("main.md");
  // Replace "cat" then "dog ran" with 8 chars between, then 9 chars between
  text.delete(4, 3); // "cat"
  text.insert(4, "tabby");
  text.delete(19, 7); // "dog ran"
  text.insert(19, "bird");
  branch.commit();

  const tip = branch.frontiers();

  const proposalData = {
    base,
    tip,
    bytes
  };

  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Two changes separated by 9 chars should be TWO hunks
  assert.equal(hunks.length, 2, "changes separated by 9 chars = two hunks");
}

// Test: Trailing retain does not extend a hunk
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  room.getText("main.md").insert(0, "The cat ran.");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  const text = branch.getText("main.md");
  // Only one edit at the very end
  text.delete(8, 3); // "ran"
  text.insert(8, "sprinted");
  branch.commit();

  const tip = branch.frontiers();

  const proposalData = {
    base,
    tip,
    bytes
  };

  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // One edit = one hunk, trailing retain doesn't extend it
  assert.equal(hunks.length, 1, "one edit = one hunk");
  assert.equal(hunks[0].deleted, 3, "hunk deletes 'ran'");
  assert.equal(hunks[0].inserted, "sprinted", "hunk inserts 'sprinted'");
}

// Test: Hunks are numbered across files, not restarted per file
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);

  room.getText("file1.md").insert(0, "The cat sat.");
  room.getText("file2.md").insert(0, "The dog ran.");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  // Edit in file1
  branch.getText("file1.md").delete(4, 3);
  branch.getText("file1.md").insert(4, "tabby");

  // Edit in file2
  branch.getText("file2.md").delete(4, 3);
  branch.getText("file2.md").insert(4, "bird");

  branch.commit();

  const tip = branch.frontiers();

  const proposalData = {
    base,
    tip,
    bytes
  };

  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Two hunks: one per file, but numbered 0 and 1 globally
  assert.equal(hunks.length, 2, "one hunk per file");
  assert.equal(hunks[0].index, 0, "first hunk is index 0");
  assert.equal(hunks[1].index, 1, "second hunk is index 1, not restarted");
}

console.log("proposals: all parity tests passed");
