import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";

// The controller owns the list, so it is a `.svelte.js` module and is
// compiled before it is imported. What the checks below call `comments` and
// `unconfirmed` is its own state, read rather than handed to it.
const { createAnnotations } = await loadRunes(
  new URL("../../src/lib/reader/annotations.svelte.js", import.meta.url),
);

const stored = new Map();
globalThis.localStorage = {
  getItem: (key) => stored.get(key) ?? null,
  setItem: (key, value) => stored.set(key, value),
};
let paints = 0;
const sent = [], anchored = [];
const annotations = createAnnotations({
  slug: "annotations-check",
  anchor: (items) => anchored.push(...items),
  repaint: () => paints++,
  send: (message) => sent.push(message),
});
const view = annotations.state;

annotations.comment({ exact: "selected words" }, {
  motivation: "commenting", body: "Check this", proposed: "must not travel",
}, "Local name");
const draft = view.comments[0];
assert.equal(draft.pending, true);
assert.equal(draft.creator, "Local name");
assert.equal(sent[0].creator, undefined);
assert.equal(sent[0].proposed, undefined);
assert.equal(draft.proposed, undefined);
assert.equal(anchored.length, 1);
assert.equal(view.unconfirmed.length, 1);
const acknowledged = {
  type: "comment", temp_id: draft.temp_id,
  comment: { id: "confirmed", creator: "Server name", body: "Check this", replies: [] },
};
annotations.receive(acknowledged);
annotations.receive(acknowledged);
assert.equal(view.comments.length, 1, "a repeated acknowledgement cannot duplicate the row");
assert.equal(view.comments[0], draft, "the optimistic row retains its anchor and identity");
assert.equal(draft.creator, "Server name");
assert.equal(draft.pending, false);
assert.equal(draft.deletable, true);
assert.equal(view.unconfirmed.length, 0);

// External broadcasts need no temporary ID and cannot replace a confirmed row.
const remote = { type: "comment", comment: { id: "remote", body: "Other comment", replies: [] } };
annotations.receive(remote);
annotations.receive(remote);
assert.deepEqual(view.comments.map((item) => item.id), ["confirmed", "remote"]);
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
assert.equal(view.unconfirmed.length, 0);
assert.equal(sent[1].creator, undefined);

annotations.reply(draft, "Unconfirmed reply", "Name");
const failed = sent.at(-1).temp_id;
annotations.outbox.failed(failed, "Connection lost");
annotations.removePending(failed);
assert.equal(view.comments[0].replies.length, 2);
assert.equal(view.unconfirmed.length, 1, "rollback preserves the draft for an explicit retry");
annotations.outbox.retry(failed, (message) => sent.push(message));
assert.equal(sent.at(-1).temp_id, failed, "retry retains the idempotency key");
annotations.discard(failed);
assert.equal(view.unconfirmed.length, 0);

annotations.comment({ exact: "replace me" }, { motivation: "editing", body: "", proposed: "replacement" }, "Name");
const suggestion = view.comments.at(-1);
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
assert.equal(view.comments.includes(suggestion), false);
assert.equal(sent.at(-1).type, "delete");
annotations.receive({ type: "refine", comment_id: draft.id,
  comment: { id: draft.id, body: "Refined response", replies: draft.replies } });
annotations.receive({ type: "refine", comment_id: draft.id,
  comment: { id: draft.id, body: "Refined response", replies: draft.replies } });
assert.equal(view.comments.find((item) => item.id === draft.id)?.body, "Refined response");
assert.equal(view.comments.find((item) => item.id === draft.id)?.pending, false);
annotations.receive({ type: "delete", comment_id: "remote" });
assert.equal(view.comments.length, 1);
assert.equal(annotations.receive({ type: "doc-state" }), false);

// The recorded position travels with the quotation and survives the
// optimistic write and the authoritative confirmation without creating a
// second row.
annotations.comment({ exact: "a passage", position: 7, prefix: "Before ", suffix: "after" },
  { motivation: "commenting", body: "Say something about this" }, "Name");
const placed = view.comments.at(-1);
assert.equal(sent.at(-1).position, 7);
assert.equal(sent.at(-1).exact, "a passage");
annotations.receive({ type: "comment", temp_id: placed.temp_id,
  comment: { id: "placed", position: 7, exact: "a passage", replies: [] } });
assert.equal(view.comments.at(-1), placed);
assert.equal(placed.id, "placed");

annotations.comment({ exact: "colored passage" },
  { motivation: "highlighting", body: "", color: "#AAbbCC" }, "Name");
const highlight = view.comments.at(-1);
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
// Where the passages got to after an edit. The server resolves the whole
// document at once and sends one frame for the pass; a card that never took
// it would keep showing where a passage was when the page was opened.
{
  const moved = view.comments.at(-1);
  const before = anchored.length;
  const repaints = paints;
  assert.equal(
    annotations.receive({
      type: "attachments",
      attachments: [
        { comment_id: moved.id, attachment: { status: "modified", resolved_range_utf16: [10, 20] } },
        { comment_id: "no-such-comment", attachment: { status: "deleted" } },
      ],
    }),
    true,
  );
  assert.deepEqual(moved.attachment, { status: "modified", resolved_range_utf16: [10, 20] });
  assert.ok(anchored.length > before, "a new attachment re-places the passage");
  assert.ok(paints > repaints);
  // A pass naming nothing this browser holds changes nothing and repaints
  // nothing, but is still this module's message to have swallowed.
  const quiet = paints;
  assert.equal(
    annotations.receive({ type: "attachments", attachments: [{ comment_id: "gone", attachment: {} }] }),
    true,
  );
  assert.equal(paints, quiet);
}

// An agent edits annotations in batches: several added, one deleted, one
// reworded, all in one act. There is no per-comment event for that, so the
// room states the whole list -- and a draft this browser is still holding must
// survive being told where everyone else ended up.
{
  annotations.comment({ exact: "still mine" }, { motivation: "commenting", body: "Not sent yet" }, "Local name");
  const draft = view.comments.at(-1);
  assert.equal(draft.pending, true);
  // Earlier checks left drafts of their own behind; every one of them is
  // this browser's and must come through.
  const held = view.comments.filter((item) => item.pending).map((item) => item.id);
  assert.ok(held.includes(draft.id));
  const before = anchored.length;
  const repaints = paints;
  assert.equal(
    annotations.receive({
      type: "comments",
      annotation_revision: 42,
      comments: [
        { id: "from-the-agent", body: "One the agent wrote", replies: [] },
        { id: "another", body: "And another", replies: [] },
      ],
    }),
    true,
  );
  assert.deepEqual(
    view.comments.map((item) => item.id),
    ["from-the-agent", "another", ...held],
    "the room's list, with this browser's unconfirmed drafts still on the end",
  );
  assert.ok(anchored.length > before, "the stated list is placed in the document");
  assert.ok(paints > repaints);
  // And once the agent's list does name it, the draft stops being a draft
  // rather than appearing twice.
  annotations.receive({
    type: "comments",
    annotation_revision: 43,
    comments: [{ id: draft.id, body: "Not sent yet", replies: [] }],
  });
  assert.deepEqual(
    view.comments.map((item) => item.id),
    [draft.id, ...held.filter((id) => id !== draft.id)],
    "the one the agent now names is stated once, not twice",
  );
  assert.equal(view.comments[0].pending, undefined);
}

assert.ok(paints > 0);
console.log("reader-annotations: optimistic writes, authoritative reconciliation, retries and decisions passed");
