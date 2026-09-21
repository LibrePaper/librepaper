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
// The actual wire response (`Ok(json!({"type": "refine", "comment":
// comment}))` in `server/mod.rs`) carries no top-level `comment_id` at all --
// only the full, updated comment. Without falling back to `comment.id`, a
// real refine broadcast finds no row to update and is silently dropped.
annotations.receive({ type: "refine",
  comment: { id: draft.id, body: "Refined again, wire-shaped", replies: draft.replies } });
assert.equal(view.comments.find((item) => item.id === draft.id)?.body, "Refined again, wire-shaped",
  "a refine response with no comment_id field still finds its row, by comment.id");
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

// The comments are paged: what arrives is a prefix, and everything the panel
// says about the collection as a whole comes from the server's own counts.
//
// A stub for the page route. Each entry is one page; `pages[n]` answers the
// nth request of the current script, so a traversal and a refresh can be
// scripted independently of how the controller decides to make them.
let served = [];
let requests = [];
let failNext = 0;
globalThis.fetch = async (url) => {
  requests.push(String(url));
  if (failNext > 0) {
    failNext -= 1;
    return { ok: false, status: 503, json: async () => ({ error: "the catalogue is unavailable" }) };
  }
  const body = served.shift();
  if (!body) throw new Error(`no page scripted for ${url}`);
  return { ok: true, status: 200, json: async () => body };
};
const page = (comments, { next = null, complete = !next, total, open = total, replies = 0 } = {}) => ({
  comments,
  next_cursor: next,
  complete,
  state: { total, open, replies, revision: `r${total}.${comments.length}` },
});

{
  // A fresh controller, so the drafts left behind above do not have to be
  // reasoned about alongside the traversal.
  const seen = [];
  let painted = 0;
  const paged = createAnnotations({
    slug: "paged-check",
    anchor: (items) => seen.push(...items),
    repaint: () => painted++,
    send: () => {},
  });
  const held = paged.state;
  assert.equal(held.page.loading, true, "nothing has arrived yet");

  // `hello` seeds the first page and what is behind it.
  paged.seed(
    [{ id: "a", body: "first", replies: [], reply_total: 0 }],
    { total: 3, open: 2, replies: 0, revision: "r1", complete: false, next_cursor: "cursor-1" },
  );
  assert.equal(held.page.loading, false);
  assert.equal(held.page.total, 3, "the count is the catalogue's, not the list's");
  assert.equal(held.page.open, 2);
  assert.equal(held.page.complete, false);
  assert.equal(held.comments.length, 1);

  // A failed "load more" leaves every comment on screen and offers a reason.
  failNext = 1;
  await paged.loadMore();
  assert.equal(held.comments.length, 1, "a failed page changes nothing on screen");
  assert.match(held.page.error, /unavailable/);
  assert.equal(held.page.busy, false);
  assert.equal(held.page.cursor, "cursor-1", "and the traversal has not advanced");

  // Retrying appends.
  served = [page([{ id: "b", body: "second", replies: [], reply_total: 0 }], { total: 3, open: 2 })];
  await paged.loadMore();
  assert.deepEqual(held.comments.map((item) => item.id), ["a", "b"]);
  assert.equal(held.page.error, "");
  assert.equal(held.page.complete, true, "the server said so; it was not inferred");
  assert.equal(held.page.pages, 2);

  // An agent's batch: no rows on the wire, just the new shape of the
  // collection and a re-read of the pages this browser is holding.
  served = [
    page([{ id: "x", body: "agent one", replies: [], reply_total: 0 }], { total: 2, next: "c", complete: false }),
    page([{ id: "y", body: "agent two", replies: [], reply_total: 0 }], { total: 2 }),
  ];
  assert.equal(
    paged.receive({ type: "comments-changed", state: { total: 2, open: 2, replies: 0, revision: "r9" } }),
    true,
  );
  assert.equal(held.page.total, 2, "the frame's counts apply at once");
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(held.comments.map((item) => item.id), ["x", "y"], "the held prefix was re-read");

  // A response from a superseded request is dropped rather than applied over
  // newer state: a refresh issued while a "load more" is in flight wins.
  served = [
    page([{ id: "fresh", body: "after", replies: [], reply_total: 0 }], { total: 1 }),
  ];
  const stale = paged.loadMore();
  await paged.refresh();
  await stale;
  assert.deepEqual(held.comments.map((item) => item.id), ["fresh"], "the older response did not land");

  // One thread's replies are their own traversal, and a repeated page cannot
  // duplicate a reply.
  // The controller has two pages held from the refresh check above; reset
  // that fixture's held-page count so this direct seed is synchronous.
  held.page.pages = 1;
  paged.seed(
    [{ id: "deep", body: "a thread", replies: [{ id: "r1", body: "one" }], reply_total: 3 }],
    { total: 1, open: 1, replies: 3, revision: "r1", complete: true },
  );
  const thread = held.comments[0];
  thread.reply_cursor = "reply-cursor";
  served = [
    { replies: [{ id: "r2", body: "two" }], next_cursor: "reply-2", complete: false, total: 3 },
    { replies: [{ id: "r2", body: "two" }, { id: "r3", body: "three" }], next_cursor: null, complete: true, total: 3 },
  ];
  await paged.loadReplies(thread);
  assert.deepEqual(thread.replies.map((reply) => reply.id), ["r1", "r2"]);
  await paged.loadReplies(thread);
  assert.deepEqual(
    thread.replies.map((reply) => reply.id),
    ["r1", "r2", "r3"],
    "a reply already held is not added twice",
  );
  assert.equal(thread.reply_cursor, null, "and the thread says it is whole");

  // Events move the authoritative counts rather than recounting the rows.
  const before = held.page.total;
  paged.receive({
    type: "comment",
    comment: { id: "new-one", body: "arrived live", replies: [], resolved: false },
  });
  assert.equal(held.page.total, before + 1);
  paged.receive({ type: "delete", comment_id: "new-one",
    state: { total: before, open: 1, replies: 0, revision: "after-delete" } });
  assert.equal(held.page.total, before, "delete uses the catalogue count even after an optimistic removal");
  // A mutation for a row outside the loaded prefix still updates the totals.
  paged.receive({ type: "resolve", comment_id: "unloaded", resolved: true,
    state: { total: before, open: 0, replies: 0, revision: "after-resolve" } });
  assert.equal(held.page.open, 0, "unloaded mutations carry authoritative counts");

  // Re-seeding while a reply request is in flight invalidates the request,
  // but leaves the stable comment usable for another attempt.
  let releaseReplies;
  const reconnect = createAnnotations({
    slug: "reconnect-check", anchor: () => {}, repaint: () => {}, send: () => {},
  });
  reconnect.seed(
    [{ id: "thread", replies: [], reply_cursor: "old" }],
    { total: 1, open: 1, replies: 1, revision: "r1", complete: true },
  );
  const reconnectThread = reconnect.state.comments[0];
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (String(url).includes("/replies?")) {
      await new Promise((resolve) => { releaseReplies = resolve; });
      return { ok: true, status: 200, json: async () => ({ replies: [], complete: false, next_cursor: "new" }) };
    }
    return previousFetch(url);
  };
  const inFlight = reconnect.loadReplies(reconnectThread);
  while (!releaseReplies) await new Promise((resolve) => setTimeout(resolve, 0));
  reconnect.seed(
    [{ id: "thread", replies: [], reply_cursor: "old" }],
    { total: 1, open: 1, replies: 1, revision: "r2", complete: true },
  );
  assert.equal(reconnectThread.repliesBusy, false, "reconnect releases the old reply request state");
  releaseReplies();
  await inFlight;
  globalThis.fetch = previousFetch;
  assert.equal(reconnectThread.repliesBusy, false);
  assert.ok(requests.every((url) => url.startsWith("/api/documents/paged-check/")));
}


assert.ok(paints > 0);
console.log("reader-annotations: optimistic writes, authoritative reconciliation, bounded paging, retries and decisions passed");
