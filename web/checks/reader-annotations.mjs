import assert from "node:assert/strict";
import { createAnnotations } from "../src/lib/reader/annotations.js";

const stored = new Map();
globalThis.localStorage = {
  getItem: (key) => stored.get(key) ?? null,
  setItem: (key, value) => stored.set(key, value),
};
let comments = [], unconfirmed = [], paints = 0;
const sent = [], anchored = [];
const annotations = createAnnotations({
  slug: "annotations-check",
  list: () => comments,
  update: (next) => { comments = next; },
  anchor: (items) => anchored.push(...items),
  repaint: () => paints++,
  send: (message) => sent.push(message),
  changed: (items) => { unconfirmed = items; },
});

annotations.comment({ exact: "selected words" }, {
  motivation: "commenting", body: "Check this", proposed: "must not travel",
}, "Local name");
const draft = comments[0];
assert.equal(draft.pending, true);
assert.equal(draft.creator, "Local name");
assert.equal(sent[0].creator, undefined);
assert.equal(sent[0].proposed, undefined);
assert.equal(draft.proposed, undefined);
assert.equal(anchored.length, 1);
assert.equal(unconfirmed.length, 1);
const acknowledged = {
  type: "comment", temp_id: draft.temp_id,
  comment: { id: "confirmed", creator: "Server name", body: "Check this", replies: [] },
};
annotations.receive(acknowledged);
annotations.receive(acknowledged);
assert.equal(comments.length, 1, "a repeated acknowledgement cannot duplicate the row");
assert.equal(comments[0], draft, "the optimistic row retains its anchor and identity");
assert.equal(draft.creator, "Server name");
assert.equal(draft.pending, false);
assert.equal(draft.deletable, true);
assert.equal(unconfirmed.length, 0);

// External broadcasts need no temporary ID and cannot replace a confirmed row.
const remote = { type: "comment", comment: { id: "remote", body: "Other comment", replies: [] } };
annotations.receive(remote);
annotations.receive(remote);
assert.deepEqual(comments.map((item) => item.id), ["confirmed", "remote"]);
assert.equal(anchored.length, 2);

annotations.reply(draft, "A reply", "Local reply name");
const reply = draft.replies[0];
const event = {
  type: "reply", comment_id: draft.id, temp_id: reply.temp_id,
  reply: { id: "confirmed-reply", body: "A reply", creator: "Server reply name" },
};
annotations.receive(event);
annotations.receive(event);
annotations.receive({ type: "reply", comment_id: draft.id, reply: { id: "remote-reply", body: "Remote reply" } });
assert.equal(draft.replies.length, 2);
assert.equal(reply.creator, "Server reply name");
assert.equal(unconfirmed.length, 0);
assert.equal(sent[1].creator, undefined);

annotations.reply(draft, "Unconfirmed reply", "Name");
const failed = sent.at(-1).temp_id;
annotations.outbox.failed(failed, "Connection lost");
annotations.removePending(failed);
assert.equal(comments[0].replies.length, 2);
assert.equal(unconfirmed.length, 1, "rollback preserves the draft for an explicit retry");
annotations.outbox.retry(failed, (message) => sent.push(message));
assert.equal(sent.at(-1).temp_id, failed, "retry retains the idempotency key");
annotations.discard(failed);
assert.equal(unconfirmed.length, 0);

annotations.comment({ exact: "replace me" }, { motivation: "editing", body: "", proposed: "replacement" }, "Name");
const suggestion = comments.at(-1);
assert.equal(sent.at(-1).proposed, "replacement");
suggestion.deciding = "accept";
annotations.receive({ type: "accept", comment_id: suggestion.id, resolved_at: "today", resolved_in: "sha" });
assert.equal(suggestion.outcome, "accepted");
assert.equal(suggestion.deciding, undefined);
annotations.receive({ type: "resolve", comment_id: suggestion.id, resolved: false, resolved_at: null });
assert.equal(suggestion.outcome, "");
annotations.resolve(suggestion);
assert.equal(suggestion.resolved, true);
assert.equal(sent.at(-1).type, "resolve");
annotations.delete(suggestion);
assert.equal(comments.includes(suggestion), false);
assert.equal(sent.at(-1).type, "delete");
annotations.receive({ type: "refine", comment_id: draft.id,
  comment: { id: draft.id, body: "Refined response", replies: draft.replies } });
annotations.receive({ type: "refine", comment_id: draft.id,
  comment: { id: draft.id, body: "Refined response", replies: draft.replies } });
assert.equal(comments.find((item) => item.id === draft.id)?.body, "Refined response");
assert.equal(comments.find((item) => item.id === draft.id)?.pending, false);
annotations.receive({ type: "delete", comment_id: "remote" });
assert.equal(comments.length, 1);
assert.equal(annotations.receive({ type: "y-state" }), false);

// Annotation appearance and zero-width anchors survive the optimistic write,
// retry payload, and authoritative confirmation without creating a second row.
annotations.comment({ exact: "", point: true, position: 7, prefix: "Before ", suffix: "after" },
  { motivation: "commenting", body: "Insert a reference here" }, "Name");
const point = comments.at(-1);
assert.equal(point.point, true);
assert.equal(sent.at(-1).position, 7);
assert.equal(sent.at(-1).exact, "");
annotations.receive({ type: "comment", temp_id: point.temp_id,
  comment: { id: "point", point: true, position: 7, exact: "", replies: [] } });
assert.equal(comments.at(-1), point);
assert.equal(point.id, "point");

annotations.comment({ exact: "colored passage" },
  { motivation: "highlighting", body: "", color: "#AAbbCC" }, "Name");
const highlight = comments.at(-1);
assert.equal(highlight.color, "#aabbcc");
assert.equal(sent.at(-1).color, "#aabbcc");
const highlightId = sent.at(-1).temp_id;
annotations.outbox.failed(highlightId, "Connection lost");
annotations.outbox.retry(highlightId, (message) => sent.push(message));
assert.equal(sent.at(-1).color, "#aabbcc");
assert.equal(sent.at(-1).temp_id, highlightId);
annotations.comment({ exact: "invalid color" },
  { motivation: "commenting", body: "Note", color: "url(https://example.com)" }, "Name");
assert.equal(sent.at(-1).color, undefined);
annotations.comment({ exact: "suggestion" },
  { motivation: "editing", proposed: "replacement", color: "#aabbcc" }, "Name");
assert.equal(sent.at(-1).color, undefined, "suggestions keep their own review styling");
assert.ok(paints > 0);
console.log("reader-annotations: optimistic writes, authoritative reconciliation, retries and decisions passed");
