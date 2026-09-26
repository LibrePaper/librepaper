import assert from "node:assert/strict";
import { LoroDoc } from "loro-crdt";
import { createProposals } from "../../src/lib/proposals.js";

// This test asserts that the browser groups hunks exactly as the server does.
// The server's grouping is in crates/librepaper/src/document/hunks.rs.
// A mismatch means "decline the second hunk" reverts something else.

const AUTHOR = 2n;
const OWNER = 1n;

// Test: Two changes separated by 10 retained characters are TWO hunks
// "The cat sat. The dog ran."
//  0123456789012345678901234
//      cat (positions 4-6, 3 chars)
//                  dog ran (positions 17-23, 7 chars)
// Gap between: " sat. The " (positions 7-16, 10 chars)
// Since 10 > 8, they are TWO hunks
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  const atFork = room.oplogVersion();
  room.getText("main.md").insert(0, "The cat sat. The dog ran.");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  // Apply later edit first so earlier offsets hold
  const text = branch.getText("main.md");
  text.delete(17, 7); // delete "dog ran"
  text.insert(17, "dog sprinted");
  text.delete(4, 3); // delete "cat"
  text.insert(4, "tabby");
  branch.commit();

  const tip = branch.frontiers();
  const bytes = branch.export({ mode: "update", from: atFork });

  const proposalData = { base, tip, bytes };
  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Gap of 10 chars > 8, so two hunks
  assert.equal(hunks.length, 2, "changes separated by 10 chars = two hunks");
}

// Test: Two changes separated by 8 retained characters are ONE hunk
// "The catXXXXXXX dog ran."
//  01234567891011121314151617
//      cat (positions 4-6, 3 chars)
//             XXXXXXX (positions 7-13, 7 chars padding)
//                     dog ran (positions 14-20, 7 chars)
// Gap between: "XXXXXXX " (positions 7-13, 7 chars, so including the space after it = 8 total)
// Wait, let me recount: "The catXXXXXXX dog ran."
// "The " = 4, "cat" = 3, "XXXXXXX " = 8, "dog ran" = 7. Total = 4+3+8+7 = 22
// The gap in the diff will be after deleting "cat" and inserting "tabby" (net +2 chars)
// So the edits need to be positioned right. Let me make a simpler case:
// "ab12345678cd" - delete "ab", insert "xy", delete "cd", insert "zw"
// Gap = "12345678" = 8 chars, so one hunk
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  const atFork = room.oplogVersion();
  room.getText("main.md").insert(0, "ab12345678cd");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  const text = branch.getText("main.md");
  // Apply later edit first
  text.delete(10, 2); // delete "cd"
  text.insert(10, "zw");
  text.delete(0, 2); // delete "ab"
  text.insert(0, "xy");
  branch.commit();

  const tip = branch.frontiers();
  const bytes = branch.export({ mode: "update", from: atFork });

  const proposalData = { base, tip, bytes };
  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Gap of 8 chars = one hunk
  assert.equal(hunks.length, 1, "changes separated by 8 chars = one hunk");
}

// Test: Two changes separated by 1 retained character are ONE hunk
// "aXbc" - delete "a", insert "y", delete "bc", insert "zw"
// Gap = "X" = 1 char, so one hunk
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  const atFork = room.oplogVersion();
  room.getText("main.md").insert(0, "aXbc");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  const text = branch.getText("main.md");
  text.delete(2, 2); // delete "bc"
  text.insert(2, "zw");
  text.delete(0, 1); // delete "a"
  text.insert(0, "y");
  branch.commit();

  const tip = branch.frontiers();
  const bytes = branch.export({ mode: "update", from: atFork });

  const proposalData = { base, tip, bytes };
  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Gap of 1 char <= 8, so one hunk
  assert.equal(hunks.length, 1, "changes separated by 1 char = one hunk");
}

// Test: Two changes separated by 9 retained characters are TWO hunks
// "ab123456789cd" - delete "ab", insert "xy", delete "cd", insert "zw"
// Gap = "123456789" = 9 chars, so two hunks
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  const atFork = room.oplogVersion();
  room.getText("main.md").insert(0, "ab123456789cd");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  const text = branch.getText("main.md");
  text.delete(11, 2); // delete "cd"
  text.insert(11, "zw");
  text.delete(0, 2); // delete "ab"
  text.insert(0, "xy");
  branch.commit();

  const tip = branch.frontiers();
  const bytes = branch.export({ mode: "update", from: atFork });

  const proposalData = { base, tip, bytes };
  const proposals = createProposals({
    session: { doc: room },
    send: () => {},
    mayEdit: true
  });

  const hunks = proposals.hunksOf(proposalData);

  // Gap of 9 chars > 8, so two hunks
  assert.equal(hunks.length, 2, "changes separated by 9 chars = two hunks");
}

// Test: Hunks are numbered across files, not restarted per file
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  const atFork = room.oplogVersion();

  room.getText("file1.md").insert(0, "The cat sat.");
  room.getText("file2.md").insert(0, "The dog ran.");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);

  // Edit in each file
  branch.getText("file1.md").delete(4, 3);
  branch.getText("file1.md").insert(4, "tabby");
  
  branch.getText("file2.md").delete(4, 3);
  branch.getText("file2.md").insert(4, "bird");

  branch.commit();

  const tip = branch.frontiers();
  const bytes = branch.export({ mode: "update", from: atFork });

  const proposalData = { base, tip, bytes };
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

// A hunk that bridges a short retain reports the whole new side, bridged text
// included. The grouping is what the server does -- one decision, not two --
// but the words shown for it have to be the words that were typed: "cat sat"
// becoming "owl ran" must not read as "owlran" in the Changes panel or in the
// insertion drawn over the text.
{
  const room = new LoroDoc();
  room.setPeerId(OWNER);
  const atFork = room.oplogVersion();
  room.getText("main.md").insert(0, "The cat sat on the mat.");
  room.commit();

  const base = room.frontiers();
  const branch = room.fork();
  branch.setPeerId(AUTHOR);
  const text = branch.getText("main.md");
  text.delete(8, 3); // "sat" -> "ran"
  text.insert(8, "ran");
  text.delete(4, 3); // "cat" -> "owl"
  text.insert(4, "owl");
  branch.commit();

  const proposals = createProposals({ session: { doc: room }, send: () => {}, mayEdit: true });
  const hunks = proposals.hunksOf({
    base,
    tip: branch.frontiers(),
    bytes: branch.export({ mode: "update", from: atFork }),
  });

  assert.equal(hunks.length, 1, "a one-word gap is one decision");
  assert.equal("The cat sat on the mat.".slice(hunks[0].start, hunks[0].start + hunks[0].deleted), "cat sat");
  assert.equal(hunks[0].inserted, "owl ran", "the retained space is part of the new side");
}

console.log("proposals: parity tests passed");
