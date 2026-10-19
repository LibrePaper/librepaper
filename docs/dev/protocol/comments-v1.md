# Bounded comment transport (`librepaper.comments.v1`)

Comments and replies use bounded HTTP pages plus a room socket for the first
page and live events. There is no endpoint or frame that returns the whole
collection. `librepaper export` exports project source and assets; it does not
export comments.

## HTTP pages

- `GET /api/documents/{slug}/comments?cursor=&limit=` reads annotations.
- `GET /api/documents/{slug}/comments/{comment_id}/replies?cursor=&limit=`
  reads one thread's replies.
- Both responses include `version: 1`, `protocol: "librepaper.comments.v1"`,
  `complete`, `next_cursor`, and `oversize`. Annotation pages also include
  authoritative `state` counts (`total`, `open`, `replies`) plus a `revision`;
  reply pages include `comment_id`, `replies`, and the thread `total`.
- An annotation page includes at most ten replies per comment by default.
  Each comment has `reply_total` and `reply_cursor` when more replies exist.
  Thread pages default to 50 replies.
- `complete` is determined by a bounded sizing query, not by whether a page is
  short. `next_cursor` is null only when the traversal is complete.

| Limit | Value |
|---|---:|
| Annotation page default / maximum | 50 / 200 rows |
| Replies previewed per annotation | 10 |
| Thread page default / maximum | 50 / 200 rows |
| Encoded page content budget | 512 KiB |

If the first row alone exceeds the byte budget, the server returns that row
and sets `oversize: true`; this keeps the rest of the traversal reachable.
The limit is on transport page size, not on how many comments or replies a
document may store. Widths are sized before full rows are fetched; only rows
that fit are loaded, except for the one-row oversize case.

## Cursors and consistency

- Annotation and reply order is `(created_at, id)` ascending. The stable ID
  tie-breaker handles rows with equal timestamps.
- A cursor is base64url JSON containing protocol version, document ID,
  optional thread ID, viewer option key, timestamp in microseconds, and row
  ID. It binds a position to its document and thread.
- Authorization is checked on every request before the cursor is decoded. A
  cursor grants no access by itself.
- Each page's sizing and row reads share a short repeatable-read transaction.
  No transaction or snapshot is held across page requests; this is a bounded
  traversal, not a point-in-time snapshot.
- Rows do not move in the ordering, so keyset continuation does not repeat a
  returned row. Concurrent creates, deletes, edits, and resolution changes
  can affect later pages. `state.revision` lets clients detect collection
  changes between reads; it does not freeze the traversal.

## Room socket

- After registering a socket as a subscriber, the server sends
  `{"type":"hello","comments":[...first page...],"state":{...}}`.
- Individual mutations are sent as bounded `comment`, `reply`, `resolve`,
  `edit`, `delete`, `refine`, `accept`, or `reject` events. Events carry viewer
  specific state counts.
- `edit` changes the body of a comment or a reply. Only its author may; the
  owner may delete but not edit. No history is kept.
  - Client frame, also accepted by `POST /api/documents/{slug}/comments`:
    `{"type":"edit","comment_id":...,"reply_id":...,"body":...}`. Omit
    `reply_id` to edit the comment itself.
  - Event: `{"type":"edit","comment_id":...,"reply_id":...|null,"body":...,`
    `"edited":...,"state":{...}}`. It carries no comment row.
  - Comments and replies gain `edited` once edited. Replies carry `mine`.
- `comments-changed` carries updated state counts but no comment rows. Clients
  re-read pages when an event describes a batch or other change that cannot
  be represented by a single row event.

The browser initially holds the first page and fetches more only on request.
It keeps counts from `state`, deduplicates overlapping live events by comment
ID, and can refresh the pages already loaded after `comments-changed` or a
new `hello`. Refresh is capped at eight pages; beyond that, the loaded list
remains partial. A failed page request leaves loaded comments visible and
offers retry.

## Attachment

A comment's `attachment` says where its passage is now. The server derives it from the comment's anchor record on each read and never stores it. Callers with editor role or above get `attachment` and `original_anchor` on comment pages, in `hello`, in events and through agent reads. Readers and commenters get neither. A comment about the whole document has no record and no attachment. The room sends no `attachments` frame. Editors derive their own attachments after each document change.

- `status` is one of `exact`, `modified`, `pending_deletion`, `deleted` or `unresolved`. `unresolved` also covers a comment with no record yet.
- `resolved_range_utf16` is `[from, to]` in UTF-16 units. It is present only for `exact` and `modified`.
- `resolved_file_id` is the file the passage is in now. It differs from `original_anchor.file_id` after a move to another file. It is absent when no file is resolved.
- `tree_digest` is the projection digest the attachment was derived against. An agent uses the attachment only when this matches the snapshot the agent read.
- `diagnostic` is `removed_file` when the record names a file that is gone or is no longer a text. `cleared_history` is reserved, and nothing sets it yet.

## Agent reads

`document_read` stores an immutable source view. Comment operations read the
live catalogue and retain their own optimistic-concurrency checks. An agent
`thread` query returns a bounded page with `complete` and `next_cursor`; the
result envelope carries `comment_revision`. Continuation cursors bind to that
revision, so a changed collection requires a fresh query. `comment_version`
is based on the annotation row, total reply count, and newest reply timestamp;
it does not depend on the number of replies in a page.

Implemented in [room/comments.rs](../../../crates/librepaper-room/src/room/comments.rs),
[room/derive.rs](../../../crates/librepaper-room/src/room/derive.rs),
[documents.rs](../../../crates/librepaper-server/src/server/documents.rs),
[anchors.js](../../../web/src/lib/anchors.js),
[annotations.svelte.js](../../../web/src/lib/reader/annotations.svelte.js).
