import assert from "node:assert/strict";
import { LoroDoc, LoroText } from "loro-crdt";
import { createProposals } from "../../src/lib/proposals.js";

// A draft takes in what others write while it is open, and its hunks stay
// the author's alone: the base moves past what was absorbed.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const files = room.getMap("files");
  const text = files.setContainer("f1", new LoroText());
  text.insert(0, "The cat sat. The dog ran.");
  room.commit();

  const sent = [];
  const proposals = createProposals({ session: { doc: room }, send: (m) => sent.push(m), mayEdit: true });
  proposals.start();
  proposals.apply({ type: "proposal-opened", proposal_id: "p1" });

  // The author types into the branch.
  proposals.text("f1").insert(4, "big ");
  proposals.flush();
  assert.equal(sent.filter((m) => m.type === "proposal-update").length, 1);

  // A coauthor edits the room elsewhere, arriving as an import.
  const other = room.fork();
  other.setPeerId(3n);
  other.getMap("files").get("f1").insert(25, " Fast.");
  other.commit();
  room.import(other.export({ mode: "update", from: room.oplogVersion() }));
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(proposals.text("f1").toString(), "The big cat sat. The dog ran. Fast.", "the draft shows the coauthor's words");
  // Nothing new of the author's, so nothing resent.
  assert.equal(sent.filter((m) => m.type === "proposal-update").length, 1, "a remote edit alone sends nothing");

  proposals.text("f1").insert(0, "Yes. ");
  proposals.flush();
  const last = sent.at(-1);
  assert.ok(last.base, "the moved base goes with the next update");

  const decode = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
  const { decodeFrontiers } = await import("loro-crdt");
  const hunks = proposals.hunksOf({
    base: decodeFrontiers(decode(last.base)),
    tip: decodeFrontiers(decode(last.tip)),
    bytes: decode(last.update),
  });
  const inserted = hunks.map((h) => h.inserted).join("|");
  assert.ok(!inserted.includes("Fast"), `the coauthor's words are not the author's hunks: ${inserted}`);
  assert.ok(inserted.includes("Yes") && inserted.includes("big"), `the author's words are: ${inserted}`);
}
console.log("proposal-absorb: ok");
