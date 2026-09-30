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

  // The author types into the branch.
  proposals.text("f1").insert(4, "big ");
  proposals.flush();
  const open = sent.at(-1);
  assert.equal(open.type, "proposal-open");
  proposals.apply({ type: "proposal-opened", proposal_id: open.request_id, request_id: open.request_id, tip: open.base, applied_version: 1 });
  const firstUpdate = sent.at(-1);
  assert.equal(firstUpdate.type, "proposal-update");
  assert.equal(firstUpdate.expected_version, 1, "updates are guarded by the acknowledged server row version");
  proposals.apply({ type: "proposal-updated", proposal_id: open.request_id, request_id: firstUpdate.request_id, tip: firstUpdate.tip, applied_version: 2 });
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
  assert.equal(last.expected_version, 2, "the next update uses the latest acknowledged row version");

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
// Retry a retryable base/version refusal even if the room has no later import.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "A paper.");
  room.commit();
  const sent = [];
  const proposals = createProposals({
    session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; },
  });
  proposals.start();
  const id = proposals.id();
  proposals.text("f1").insert(0, "New. ");
  proposals.flush();
  const first = sent.at(-1);
  proposals.apply({ type: "error", request_id: id, message: "base is not durable yet", retry: true });
  await new Promise((resolve) => setTimeout(resolve, 1700));
  const retry = sent.at(-1);
  assert.equal(retry.type, "proposal-open", "retryable refusal retries without another source import");
  assert.equal(retry.request_id, id, "retry preserves the idempotent open key");
  assert.ok(first.base, "the first open carried its immutable base");
  assert.equal(retry.base, first.base, "the retry uses the original open base");
}
// A version conflict refreshes the compare-and-swap version without treating
// the locally submitted operations as acknowledged.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "A paper.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  proposals.text("f1").insert(0, "New. ");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const first = sent.at(-1);
  proposals.apply({ type: "error", request_id: first.request_id, version_conflict: true,
    applied_version: 4, message: "proposal row advanced" });
  await new Promise((resolve) => setTimeout(resolve, 1700));
  const retried = sent.at(-1);
  assert.equal(retried.type, "proposal-update");
  assert.equal(retried.expected_version, 4, "retry uses the server's current CAS version");
  assert.notEqual(retried.tip, open.base, "conflict recovery keeps the proposal operations pending");
}
// A UUID is available before the server opens anything. Failed and stopped
// drafts stay retryable until the server acknowledges both open and update.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "A paper.");
  room.commit();
  let online = false;
  const sent = [];
  const proposals = createProposals({
    session: { doc: room },
    mayEdit: true,
    send(message) {
      sent.push(message);
      return online ? { ok: true } : { ok: false, error: new Error("offline") };
    },
  });
  proposals.start();
  const id = proposals.id();
  assert.ok(id, "the draft id exists locally before open acknowledgement");
  assert.equal(sent.length, 0, "tracking on without edits creates no server row");
  proposals.text("f1").insert(0, "New. ");
  proposals.flush();
  assert.equal(sent.at(-1).type, "proposal-open");
  assert.equal(sent.at(-1).request_id, id);
  proposals.stop();
  assert.equal(proposals.status().pending, 1, "stopping preserves the unsent branch");

  online = true;
  proposals.reconnect();
  assert.equal(sent.at(-1).request_id, id, "reconnect retries the same create key");
  const opened = sent.find((message) => message.type === "proposal-open");
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id, tip: opened.base, applied_version: 1 });
  const update = sent.at(-1);
  assert.equal(update.type, "proposal-update");
  proposals.apply({ type: "proposal-updated", proposal_id: id, request_id: update.request_id, tip: update.tip, applied_version: 2 });
  assert.equal(proposals.status().pending, 0, "the retained branch is released only after update acknowledgement");
}
// A reconnect queries a saved row even with no pending update, so a final
// decision missed by this client can still rebase its unsent typing.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  const branchText = proposals.text("f1");
  branchText.delete(4, 3);
  branchText.insert(4, "tabby");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  branchText.insert(6, "X");

  proposals.reconnect();
  const query = sent.at(-1);
  assert.equal(query.type, "proposal-open", "reconnect checks the durable proposal outcome");
  assert.equal(query.request_id, id, "the query reuses the proposal id");
  assert.equal(query.resume, true, "known ids query status without recreating expired rows");
  proposals.apply({ type: "proposal-decided", proposal_id: id,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  assert.ok(proposals.text("f1").toString().includes("X"), "missed resolution preserves unsent typing");
  assert.ok(!proposals.text("f1").toString().includes("tabby"), "missed rejection removes the published replacement");
}
// A branch that returns to its base after it was opened is discarded instead
// of leaving a proposal with no hunks for reviewers to resolve.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "A paper.");
  room.commit();
  const sent = [];
  const proposals = createProposals({
    session: { doc: room },
    mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; },
  });
  proposals.start();
  const id = proposals.id();
  proposals.text("f1").insert(0, "New. ");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id, tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id, request_id: update.request_id, tip: update.tip, applied_version: 2 });
  proposals.text("f1").delete(0, 5);
  proposals.flush();
  const discard = sent.at(-1);
  assert.equal(discard.type, "proposal-discard", "the empty proposal row is removed");
  proposals.text("f1").insert(0, "X"); // typing after the delete request remains private
  proposals.apply({
    type: "proposal-discarded",
    proposal_id: id,
    request_id: discard.request_id,
    discarded: true,
    resolved_tip: update.tip,
    resolved_base: open.base,
  });
  assert.notEqual(proposals.id(), id, "the residual typing receives a fresh proposal id");
  assert.ok(proposals.text("f1").toString().includes("X"), "typing after the delete request survives");
  assert.equal(sent.at(-1).type, "proposal-open", "the residual branch is sent after deletion settles");
}

// When a reviewer rejects the published tip, a local edit made inside that
// proposed word survives the inverse and becomes a new proposal from live.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat.");
  room.commit();
  const sent = [];
  const proposals = createProposals({
    session: { doc: room },
    mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; },
  });
  proposals.start();
  const oldId = proposals.id();
  const branchText = proposals.text("f1");
  branchText.delete(4, 3);
  branchText.insert(4, "tabby");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: oldId, request_id: oldId, tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: oldId, request_id: update.request_id, tip: update.tip, applied_version: 2 });

  // This operation was typed after the acknowledged proposal snapshot.
  branchText.insert(6, "X");
  proposals.apply({
    type: "proposal-decided",
    proposal_id: oldId,
    hunk: 0,
    accepted: false,
    resolved: true,
    decisions: [{ hunk: 0, accepted: false }],
    resolved_tip: update.tip,
    resolved_base: open.base,
  });
  assert.notEqual(proposals.id(), oldId, "preserved remainder gets a new durable id");
  assert.ok(proposals.text("f1").toString().includes("X"), "the unsent keystroke survives rejection");
  assert.ok(!proposals.text("f1").toString().includes("tabby"), "the rejected published replacement is removed");
  assert.equal(sent.at(-1).type, "proposal-open", "the remainder opens lazily from the current room");
}
// The final decision can reach this client before the accepted source update.
// Hold it pending until the room contains the resolved proposal frontier.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  proposals.text("f1").delete(4, 3);
  proposals.text("f1").insert(4, "tabby");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: true }], resolved_tip: update.tip, resolved_base: open.base });
  assert.equal(proposals.drafting(), true, "the final event waits for its source graph");

  const sourceUpdate = Uint8Array.from(atob(update.update), (character) => character.charCodeAt(0));
  room.import(sourceUpdate);
  assert.equal(proposals.drafting(), false, "the accepted source import completes resolution");
}
// A stale rejected span must not be inversed over a coauthor replacement:
// preserve the full local branch and expose a recovery error instead.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  const branchText = proposals.text("f1");
  branchText.delete(4, 3);
  branchText.insert(4, "tabby");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  branchText.insert(6, "X"); // an unacknowledged local tail must be preserved

  const other = room.fork();
  other.setPeerId(3n);
  other.getMap("files").get("f1").delete(4, 3);
  other.getMap("files").get("f1").insert(4, "fox");
  other.commit();
  room.import(other.export({ mode: "update", from: room.oplogVersion() }));
  await new Promise((resolve) => setTimeout(resolve, 0));
  const beforeResolution = proposals.text("f1").toString();
  const sendsBeforeResolution = sent.length;

  proposals.apply({ type: "proposal-decided", proposal_id: id,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  assert.equal(proposals.status().state, "error", "unsafe inverse is surfaced for manual recovery");
  assert.equal(proposals.text("f1").toString(), beforeResolution, "the local and coauthor text is left untouched");
  assert.equal(sent.length, sendsBeforeResolution, "unsafe remainder is not silently republished");
  proposals.stop();
  proposals.start();
  const recovery = proposals.recoveryTexts().find((item) => item.id === id);
  assert.ok(recovery, "the blocked draft remains available after Editor recreation");
  assert.ok(recovery.files.some((file) => file.text.includes("X")), "the export contains the unsent tail");
  assert.equal(proposals.status().recoveryRequired, true, "recovery status survives a new active draft");
}
// Rejecting a newly created file's published text preserves a local tail in
// that file and keeps its path metadata so the remainder can be reviewed.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const source = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  source.insert(0, "A paper.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const oldId = proposals.id();
  const bib = proposals.doc().getMap("files").setContainer("f2", new LoroText());
  proposals.doc().getMap("paths").set("f2", "references.bib");
  bib.insert(0, "@article{a}");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: oldId, request_id: oldId,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: oldId,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  bib.insert(11, "X");
  proposals.apply({ type: "proposal-decided", proposal_id: oldId, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip, resolved_base: open.base });
  assert.ok(proposals.text("f2").toString().includes("X"), "the unacknowledged new-file tail survives rejection");
  assert.equal(proposals.doc().getMap("paths").get("f2"), "references.bib", "the residual file keeps its path");
}
console.log("proposal-absorb: ok");
