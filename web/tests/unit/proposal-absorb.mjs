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
// If an initial open was sent but not acknowledged, reconnect retries that
// create even when a local undo marked the now-empty draft for discard.
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
  const firstOpen = sent.at(-1);
  proposals.text("f1").delete(0, 5);
  proposals.flush();
  proposals.reconnect();
  const retry = sent.at(-1);
  assert.equal(retry.type, "proposal-open", "the pending initial open is replayed despite local undo");
  assert.equal(retry.request_id, id);
  assert.equal(retry.resume, false, "an unacknowledged create is not a status-only query");
  assert.equal(retry.base, firstOpen.base);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: firstOpen.base, applied_version: 1 });
  assert.equal(sent.at(-1).type, "proposal-discard", "the empty opened row is discarded after its ack");
}
// A status query past the outcome retention period blocks reuse of the old id
// and keeps the full local branch available through recovery export.
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
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  proposals.reconnect();
  assert.equal(sent.at(-1).resume, true);
  proposals.apply({ type: "error", request_id: id, status_unknown: true });
  assert.equal(proposals.status().recoveryRequired, true);
  assert.ok(proposals.recoveryTexts().find((draft) => draft.id === id)?.files[0].text.includes("New."));
  proposals.flush();
  assert.equal(sent.at(-1).type, "proposal-open", "an expired id is never updated or reopened");
  assert.equal(sent.at(-1).resume, true);
}
// A failed resend after undo keeps the original-create marker for a later
// reconnect, which can still obtain the ack needed to discard the empty row.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "A paper.");
  room.commit();
  let online = true;
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) {
      sent.push(message);
      return online ? { ok: true } : { ok: false, error: new Error("offline") };
    } });
  proposals.start();
  const id = proposals.id();
  proposals.text("f1").insert(0, "New. ");
  proposals.flush();
  const firstOpen = sent.at(-1);
  proposals.text("f1").delete(0, 5);
  proposals.flush();

  online = false;
  proposals.reconnect();
  assert.equal(sent.at(-1).type, "proposal-open");
  assert.equal(sent.at(-1).resume, false);
  online = true;
  proposals.reconnect();
  const secondRetry = sent.at(-1);
  assert.equal(secondRetry.type, "proposal-open", "a failed resend remains retryable");
  assert.equal(secondRetry.request_id, id);
  assert.equal(secondRetry.resume, false);
  assert.equal(secondRetry.base, firstOpen.base);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: firstOpen.base, applied_version: 1 });
  assert.equal(sent.at(-1).type, "proposal-discard", "the eventual open ack releases the queued discard");
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

// Undoing an unacknowledged open keeps its stable id through a retry, then
// removes the empty server row before accepting new typing.
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
  const branchText = proposals.text("f1");
  branchText.insert(0, "New. ");
  proposals.flush();
  const open = sent.at(-1);
  branchText.delete(0, 5);
  proposals.flush();
  proposals.apply({ type: "error", request_id: id, message: "temporary open refusal", retry: true });

  await new Promise((resolve) => setTimeout(resolve, 1600));
  const retried = sent.at(-1);
  assert.equal(retried.type, "proposal-open", "the retry resends the pending create despite empty-text discard");
  assert.equal(retried.request_id, id, "the retry preserves the client-chosen create id");
  assert.equal(retried.resume, false, "the retry remains an initial create");
  assert.equal(retried.base, open.base, "the retry keeps the immutable open base");

  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const discard = sent.at(-1);
  assert.equal(discard.type, "proposal-discard", "the open acknowledgement completes the pending empty discard");
  proposals.apply({ type: "proposal-discarded", proposal_id: id, request_id: discard.request_id,
    discarded: true, resolved_tip: open.base, resolved_base: open.base });
  assert.equal(proposals.drafting(), false, "the empty discarded branch is released");

  proposals.start();
  proposals.text("f1").insert(0, "Again. ");
  proposals.flush();
  const nextOpen = sent.at(-1);
  assert.equal(nextOpen.type, "proposal-open", "typing after the discard opens a proposal");
  assert.notEqual(nextOpen.request_id, id, "new typing gets a fresh proposal id");
}

// An opened discard retries with the same request id after a retryable error.
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
  const branchText = proposals.text("f1");
  branchText.insert(0, "New. ");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  branchText.delete(0, 5);
  proposals.flush();
  const discard = sent.at(-1);
  assert.equal(discard.type, "proposal-discard");
  proposals.apply({ type: "error", request_id: discard.request_id,
    message: "temporary discard refusal", retry: true });

  await new Promise((resolve) => setTimeout(resolve, 1600));
  const retried = sent.at(-1);
  assert.equal(retried.type, "proposal-discard", "the retry resends the outstanding discard");
  assert.equal(retried.proposal_id, id);
  assert.equal(retried.request_id, discard.request_id, "the retry keeps the discard idempotency key");
  proposals.apply({ type: "proposal-discarded", proposal_id: id, request_id: retried.request_id,
    discarded: true, resolved_tip: update.tip, resolved_base: open.base });
  assert.equal(proposals.drafting(), false, "the acknowledged discard releases the draft");
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

// A delayed update ack cannot destroy a retained draft while its empty row's
// discard acknowledgement is outstanding. Reconnect retries the same delete.
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
  const update = sent.at(-1);

  proposals.text("f1").delete(0, 5);
  proposals.flush();
  const firstDiscard = sent.at(-1);
  assert.equal(firstDiscard.type, "proposal-discard", "undo requests deletion of the now-empty row");
  proposals.stop();
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  assert.equal(proposals.status().id, id, "the retained row survives its late update ack");
  const repeatedDiscard = sent.at(-1);
  assert.equal(repeatedDiscard.type, "proposal-discard", "the pending delete is retried");
  assert.equal(repeatedDiscard.request_id, firstDiscard.request_id,
    "the retry remains idempotent with the original discard id");

  proposals.reconnect();
  const query = sent.at(-1);
  assert.equal(query.type, "proposal-open", "reconnect checks whether the row still exists");
  assert.equal(query.resume, true, "reconnect resumes the same durable row");
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: update.tip, applied_version: 2 });
  const reconnectDiscard = sent.at(-1);
  assert.equal(reconnectDiscard.type, "proposal-discard", "reconnect retries an unacknowledged deletion");
  assert.equal(reconnectDiscard.request_id, firstDiscard.request_id,
    "reconnect reuses the original discard id");
  proposals.apply({ type: "proposal-discarded", proposal_id: id,
    request_id: firstDiscard.request_id, discarded: true,
    resolved_tip: update.tip, resolved_base: open.base });
  assert.notEqual(proposals.status().id, id, "the retained row clears only after durable deletion");
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
// A local replacement overlapping the inverse is retained for manual recovery.
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
  branchText.delete(4, 5);
  branchText.insert(4, "cat");
  const beforeResolution = branchText.toString();
  const sendsBeforeResolution = sent.length;

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  assert.equal(proposals.status().recoveryRequired, true, "overlapping replacement requires recovery");
  assert.match(proposals.status().error, /local edit overlaps text being restored/);
  assert.equal(proposals.text("f1").toString(), beforeResolution, "the local replacement remains untouched");
  assert.equal(sent.length, sendsBeforeResolution, "the unsafe replacement is not auto-published");
  assert.ok(proposals.recoveryTexts().some((draft) => draft.files.some((file) => file.text === beforeResolution)));
}

// A mixed resolution imports the accepted edit and the server's rejected-hunk
// inverse together. Compare against the private pre-import frontier: a local
// restoration needs recovery, while a clean or disjoint tail resolves normally.
for (const localChange of ["restore", "clean", "tail"]) {
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat. The dog ran.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  const branchText = proposals.text("f1");
  branchText.delete(4, 3);
  branchText.insert(4, "tabby");
  const dogAt = branchText.toString().indexOf("dog");
  branchText.delete(dogAt, 3);
  branchText.insert(dogAt, "fox");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });

  if (localChange === "restore") {
    branchText.delete(4, 5);
    branchText.insert(4, "cat");
  } else if (localChange === "tail") {
    branchText.insert(branchText.toString().length, " X");
  }
  let recoveryText = branchText.toString();
  const sourceUpdate = Uint8Array.from(atob(update.update), (character) => character.charCodeAt(0));
  const resolved = room.fork();
  resolved.import(sourceUpdate);
  resolved.setPeerId(4n);
  const resolvedMain = resolved.getMap("files").get("f1");
  resolvedMain.delete(4, 5);
  resolvedMain.insert(4, "cat");
  resolved.commit();
  room.import(resolved.export({ mode: "update", from: room.oplogVersion() }));
  resolved.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));
  if (localChange === "restore") {
    branchText.insert(branchText.toString().length, " private");
    recoveryText = branchText.toString();
    proposals.flush();
    const laterCoauthor = room.fork();
    laterCoauthor.setPeerId(5n);
    const laterText = laterCoauthor.getMap("files").get("f1");
    laterText.insert(laterText.toString().length, " Coauthor.");
    laterCoauthor.commit();
    room.import(laterCoauthor.export({ mode: "update", from: room.oplogVersion() }));
    laterCoauthor.destroy?.();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  const sentBeforeDecision = sent.length;

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }, { hunk: 1, accepted: true }],
    resolved_tip: update.tip, resolved_base: open.base });
  assert.ok(room.getMap("files").get("f1").toString().startsWith("The cat sat. The fox ran."),
    "the source contains the accepted hunk and server inverse for the declined hunk");
  if (localChange === "restore") {
    assert.equal(proposals.status().recoveryRequired, true,
      "a declined local restoration blocks mixed automatic rebasing");
    assert.match(proposals.status().error, /local edit overlaps declined text/);
    assert.equal(sent.length, sentBeforeDecision, "the conflicted restoration is not auto-published");
    assert.ok(recoveryText.endsWith(" private"), "the local branch includes typing after the room inverse");
    assert.ok(proposals.recoveryTexts().some((draft) =>
      draft.files.some((file) => file.id === "f1" && file.text === recoveryText)),
    "the private restore and later typing remain downloadable after another room import");
  } else {
    assert.notEqual(proposals.status().recoveryRequired, true,
      "a server inverse alone does not create a local overlap");
    assert.equal(proposals.drafting(), localChange === "tail",
      "only an independent local tail remains as a draft");
    if (localChange === "tail") {
      assert.ok(proposals.text("f1").toString().endsWith(" X"),
        "the disjoint local tail survives the mixed resolution");
      assert.equal(sent.at(-1).type, "proposal-open", "the disjoint tail can be proposed normally");
    } else {
      assert.equal(sent.length, sentBeforeDecision, "a clean mixed resolution sends no residual proposal");
    }
  }
}

// Keep the first sent tip across reconnect when its update acknowledgement was
// lost. If the room resolves that tip before the status reply, later typing
// stays private and the rejected restoration is exported without duplication.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat. The dog ran.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  const branchText = proposals.text("f1");
  branchText.delete(4, 3);
  branchText.insert(4, "tabby");
  const dogAt = branchText.toString().indexOf("dog");
  branchText.delete(dogAt, 3);
  branchText.insert(dogAt, "fox");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.reconnect();
  assert.equal(sent.at(-1).type, "proposal-open", "reconnect queries the row after the lost update ack");
  assert.equal(sent.at(-1).resume, true, "the query resumes the existing row");

  const sourceUpdate = Uint8Array.from(atob(update.update), (character) => character.charCodeAt(0));
  const resolved = room.fork();
  resolved.import(sourceUpdate);
  resolved.setPeerId(4n);
  const resolvedMain = resolved.getMap("files").get("f1");
  resolvedMain.delete(4, 5);
  resolvedMain.insert(4, "cat");
  resolved.commit();
  room.import(resolved.export({ mode: "update", from: room.oplogVersion() }));
  resolved.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  branchText.delete(4, 5);
  branchText.insert(4, "cat");
  branchText.insert(branchText.toString().length, " private");
  const expectedRecovery = branchText.toString();
  const laterCoauthor = room.fork();
  laterCoauthor.setPeerId(5n);
  const laterText = laterCoauthor.getMap("files").get("f1");
  laterText.insert(laterText.toString().length, " Coauthor.");
  laterCoauthor.commit();
  room.import(laterCoauthor.export({ mode: "update", from: room.oplogVersion() }));
  laterCoauthor.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }, { hunk: 1, accepted: true }],
    resolved_tip: update.tip, resolved_base: open.base });
  assert.equal(proposals.status().recoveryRequired, true,
    "the missed update ack does not make a declined local restore look like server text");
  assert.ok(proposals.recoveryTexts().some((draft) =>
    draft.files.some((file) => file.id === "f1" && file.text === expectedRecovery)),
  "the private restore and late typing are preserved exactly after reconnect and two room imports");
}

// Restoring text at the zero-width gap of a published deletion also conflicts.
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
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  branchText.insert(4, "cat");

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  assert.equal(proposals.status().recoveryRequired, true, "local restoration at the rejected gap requires recovery");
  assert.equal(proposals.text("f1").toString(), "The cat sat.", "the restored text is left untouched");
}

// A nearby published deletion keeps its zero-width restore gap even though
// reviewer hunk grouping bridges it to another inverse replacement.
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
  branchText.delete(10, 3);
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  branchText.insert(10, "sat");
  const beforeResolution = branchText.toString();
  const sendsBeforeResolution = sent.length;

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  assert.equal(proposals.status().recoveryRequired, true, "restoration at a bridged inverse gap requires recovery");
  assert.equal(proposals.text("f1").toString(), beforeResolution, "the nearby restoration remains untouched");
  assert.equal(sent.length, sendsBeforeResolution, "the ambiguous restoration is not auto-published");
}

// A disjoint local replacement still rebases automatically.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat. The dog ran.");
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
  const dogAt = branchText.toString().indexOf("dog");
  branchText.delete(dogAt, 3);
  branchText.insert(dogAt, "fox");

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  assert.equal(proposals.status().recoveryRequired, false, "a disjoint replacement rebases automatically");
  assert.notEqual(proposals.id(), id, "the disjoint replacement opens as a residual proposal");
  assert.equal(proposals.text("f1").toString(), "The cat sat. The fox ran.");
  assert.equal(sent.at(-1).type, "proposal-open", "the disjoint remainder opens as a fresh proposal");
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
// Later room imports cannot erase a blocked recovery copy, while a fresh
// active draft still absorbs those same source changes.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const main = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  main.insert(0, "The cat sat.");
  const notes = room.getMap("files").setContainer("f2", new LoroText());
  room.getMap("paths").set("f2", "notes.md");
  notes.insert(0, "Notes.");
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

  const coauthor = room.fork();
  coauthor.setPeerId(3n);
  coauthor.getMap("files").get("f1").delete(4, 3);
  coauthor.getMap("files").get("f1").insert(4, "fox");
  coauthor.commit();
  room.import(coauthor.export({ mode: "update", from: room.oplogVersion() }));
  coauthor.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  proposals.apply({ type: "proposal-decided", proposal_id: id,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  const recoveryError = proposals.status().error;
  const before = proposals.recoveryTexts().find((draft) => draft.id === id);
  const preservedFile = before?.files.find((file) => file.id === "f1");
  assert.ok(preservedFile, "the stale draft exports the conflicted source file");
  proposals.stop();
  proposals.start();

  const laterCoauthor = room.fork();
  laterCoauthor.setPeerId(4n);
  laterCoauthor.getMap("files").delete("f1");
  laterCoauthor.getMap("paths").delete("f1");
  laterCoauthor.getMap("files").get("f2").insert(6, " revised");
  laterCoauthor.commit();
  room.import(laterCoauthor.export({ mode: "update", from: room.oplogVersion() }));
  laterCoauthor.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(proposals.status().recoveryRequired, true, "later imports keep recovery required");
  assert.equal(proposals.status().error, recoveryError, "later imports do not change the recovery warning");
  const after = proposals.recoveryTexts().find((draft) => draft.id === id);
  assert.deepEqual(after?.files.find((file) => file.id === "f1"), preservedFile,
    "the blocked recovery file survives a later room deletion");
  assert.equal(proposals.text("f2").toString(), "Notes. revised",
    "the fresh active draft still absorbs a coauthor edit");
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
  assert.equal(proposals.text("f2").toString(), "X", "only the unacknowledged new-file tail survives rejection");
  assert.equal(proposals.doc().getMap("paths").get("f2"), "references.bib", "the residual file keeps its path");
}
// A rejected new file with no surviving local text is removed when the
// author has a separate residual edit in another file.
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
  const id = proposals.id();
  const bib = proposals.doc().getMap("files").setContainer("f2", new LoroText());
  proposals.doc().getMap("paths").set("f2", "references.bib");
  bib.insert(0, "@article{a}");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  proposals.text("f1").insert(0, "X");
  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip, resolved_base: open.base });
  assert.equal(proposals.doc().getMap("files").get("f2"), undefined, "the emptied rejected file is removed");
  assert.equal(proposals.doc().getMap("paths").get("f2"), undefined, "its path entry is removed too");
  assert.ok(proposals.text("f1").toString().includes("X"), "the other-file tail becomes a residual proposal");
}
// A mixed resolution can remove a declined new file from the room graph while
// the author has later typing in its text container. Preserve a full export
// for manual recovery instead of silently dropping that unpublished text.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const source = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  source.insert(0, "The cat sat.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  const main = proposals.text("f1");
  main.delete(4, 3);
  main.insert(4, "tabby");
  const bib = proposals.doc().getMap("files").setContainer("f2", new LoroText());
  proposals.doc().getMap("paths").set("f2", "references.bib");
  bib.insert(0, "@article{a}");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 2 });
  // Delete and retype the final character so the visible text is unchanged,
  // but the local CRDT operation is still beyond the published tip.
  bib.delete(10, 1);
  bib.insert(10, "}");

  const decode = (value) => Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
  const { decodeFrontiers } = await import("loro-crdt");
  const hunks = proposals.hunksOf({
    base: decodeFrontiers(decode(open.base)),
    tip: decodeFrontiers(decode(update.tip)),
    bytes: decode(update.update),
  });
  const mainHunk = hunks.find((hunk) => hunk.file === "f1");
  const bibHunk = hunks.find((hunk) => hunk.file === "f2");
  assert.ok(mainHunk && bibHunk, "the proposal contains one hunk in each file");

  // Model the server's mixed result: publish the accepted main-file edit and
  // remove the declined created file's map entries.
  const resolved = room.fork();
  resolved.setPeerId(4n);
  resolved.import(decode(update.update));
  resolved.getMap("files").delete("f2");
  resolved.getMap("paths").delete("f2");
  resolved.commit();
  room.import(resolved.export({ mode: "update", from: room.oplogVersion() }));
  resolved.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [
      { hunk: mainHunk.index, accepted: true },
      { hunk: bibHunk.index, accepted: false },
    ],
    resolved_tip: update.tip,
    resolved_base: open.base,
  });
  assert.equal(proposals.status().recoveryRequired, true, "the mixed file deletion requires manual recovery");
  proposals.stop();
  proposals.start();
  const recovery = proposals.recoveryTexts().find((draft) => draft.id === id);
  assert.ok(recovery, "the unresolved draft remains exportable after Editor restart");
  assert.ok(recovery.files.some((file) => file.path === "references.bib" && file.text === "@article{a}"),
    "the recovery export preserves identity-only edits even when visible text is unchanged");
}
// A mixed resolution reaches a retained draft while no Editor is tracking it.
// Late acknowledgements and errors must preserve its recovery export.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const source = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  source.insert(0, "The cat sat.");
  room.commit();
  const sent = [];
  const proposals = createProposals({ session: { doc: room }, mayEdit: true,
    send(message) { sent.push(message); return { ok: true }; } });
  proposals.start();
  const id = proposals.id();
  const main = proposals.text("f1");
  main.delete(4, 3);
  main.insert(4, "tabby");
  const bib = proposals.doc().getMap("files").setContainer("f2", new LoroText());
  proposals.doc().getMap("paths").set("f2", "references.bib");
  bib.insert(0, "@article{a}");
  proposals.flush();
  const open = sent.at(-1);
  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 1 });
  const update = sent.at(-1);
  bib.insert(11, "X"); // This text is absent from the unacknowledged update.
  proposals.stop();

  const decode = (value) => Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
  const { decodeFrontiers } = await import("loro-crdt");
  const hunks = proposals.hunksOf({
    base: decodeFrontiers(decode(open.base)),
    tip: decodeFrontiers(decode(update.tip)),
    bytes: decode(update.update),
  });
  const mainHunk = hunks.find((hunk) => hunk.file === "f1");
  const bibHunk = hunks.find((hunk) => hunk.file === "f2");
  assert.ok(mainHunk && bibHunk, "the proposal contains one hunk in each file");

  const resolved = room.fork();
  resolved.setPeerId(4n);
  resolved.import(decode(update.update));
  resolved.getMap("files").delete("f2");
  resolved.getMap("paths").delete("f2");
  resolved.commit();
  room.import(resolved.export({ mode: "update", from: room.oplogVersion() }));
  resolved.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [
      { hunk: mainHunk.index, accepted: true },
      { hunk: bibHunk.index, accepted: false },
    ],
    resolved_tip: update.tip,
    resolved_base: open.base,
  });
  const recoveryError = proposals.status().error;
  assert.equal(proposals.status().recoveryRequired, true, "the mixed decision blocks for recovery");
  proposals.start();

  proposals.apply({ type: "proposal-opened", proposal_id: id, request_id: id,
    tip: open.base, applied_version: 3 });
  assert.equal(proposals.status().error, recoveryError, "a delayed open status keeps the recovery explanation");
  proposals.apply({ type: "proposal-updated", proposal_id: id,
    request_id: update.request_id, tip: update.tip, applied_version: 4 });
  assert.equal(proposals.status().recoveryRequired, true, "a delayed update ack keeps recovery required");
  assert.equal(proposals.status().error, recoveryError, "the delayed ack keeps the recovery explanation");
  const recovery = proposals.recoveryTexts().find((draft) => draft.id === id);
  assert.ok(recovery?.files.some((file) => file.path === "references.bib" && file.text === "@article{a}X"),
    "the delayed ack keeps the complete declined file available after Editor restart");
  const sendsBeforeLateError = sent.length;
  proposals.apply({ type: "error", request_id: id, message: "late update error", retry: true });
  assert.equal(proposals.status().error, recoveryError, "a late error keeps the recovery explanation");
  assert.equal(sent.length, sendsBeforeLateError, "a late error does not start a retry");
  await new Promise((resolve) => setTimeout(resolve, 1600));
  assert.equal(sent.length, sendsBeforeLateError, "the blocked draft does not emit a delayed retry");
  const afterLateError = proposals.recoveryTexts().find((draft) => draft.id === id);
  assert.ok(afterLateError?.files.some((file) => file.path === "references.bib" && file.text === "@article{a}X"),
    "late acknowledgements and errors retain the exact recovery file");
}
// A final rejection received before hydration must remain pending. Once the
// coauthor's overlapping replacement arrives, re-evaluate against that source
// identity and preserve the unsent tail for recovery without undoing their text.
{
  const room = new LoroDoc();
  room.setPeerId(1n);
  const text = room.getMap("files").setContainer("f1", new LoroText());
  room.getMap("paths").set("f1", "main.md");
  text.insert(0, "The cat sat.");
  room.commit();
  const session = { doc: room, joined: true };
  const sent = [];
  const proposals = createProposals({ session, mayEdit: true,
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
  const beforeResolution = branchText.toString();

  session.joined = false;
  proposals.apply({ type: "proposal-decided", proposal_id: id, resolved: true,
    decisions: [{ hunk: 0, accepted: false }], resolved_tip: update.tip,
    resolved_base: open.base });
  assert.equal(proposals.text("f1").toString(), beforeResolution,
    "a final frame during hydration does not apply an inverse to a partial room");

  const coauthor = room.fork();
  coauthor.setPeerId(3n);
  coauthor.getMap("files").get("f1").delete(4, 3);
  coauthor.getMap("files").get("f1").insert(4, "fox");
  coauthor.commit();
  room.import(coauthor.export({ mode: "update", from: room.oplogVersion() }));
  coauthor.destroy?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  session.joined = true;
  proposals.reconnect();
  assert.equal(room.getMap("files").get("f1").toString(), "The fox sat.",
    "delayed resolution keeps the overlapping coauthor replacement intact");
  assert.equal(proposals.status().recoveryRequired, true,
    "the stale proposal remainder is blocked for manual recovery");
  const recovery = proposals.recoveryTexts().find((draft) => draft.id === id);
  assert.ok(recovery?.files.some((file) => file.text.includes("X")),
    "the local typing remains available in the manual recovery export");
}
console.log("proposal-absorb: ok");
