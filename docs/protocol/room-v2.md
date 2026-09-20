# Existing LibrePaper room operations for automation

This document describes the HTTP and WebSocket contract used by headless
automation peers. The credential is the value of the x-librepaper-key header.
Clients may also send x-librepaper-automation: 1; this is required for
link-bounded operation when a machine also has a cached signed-in browser
session. The server keeps that identity for attribution and policy ceilings,
but the live link remains the complete document authority. Signed-in comments
retain the account's normal attribution; an anonymous link-only peer receives
a stable link pseudonym.

This is `librepaper.room.v2`, the protocol in `SPEC-server-is-a-log.md`. The
server stores and forwards source bytes and reads only their headers to do
so; it interprets their contents only on demand, through a bounded, evictable
cache. See that document for the reasoning behind each rule below.

## Snapshot

GET /api/documents/{slug}/snapshot requires editor or owner access and returns
one room-locked read:

    {
      "version": 1,
      "protocol": "librepaper.snapshot.v1",
      "slug": "paper",
      "title": "Paper",
      "format": "markdown",
      "main": "main.md",
      "source": "# Paper\n",
      "source_sha": "sha256-of-source-bytes",
      "tree": {"main": "main.md", "files": {}},
      "files": {},
      "texts": {"main.md": "# Paper\n"},
      "comments": [],
      "role": "editor",
      "capabilities": {"read": true, "comment": true, "edit": true}
    }

source, tree, files, texts, and comments come from the same room-locked
projection: the tree of `(path, file_id | asset_digest)` produced by the
projection algorithm at the current head, which may include text not yet
flushed to PostgreSQL. source_sha is the SHA-256 digest of the returned
source bytes. The top-level main is the live tree's main path; clients must
not use a stale main path from an older document metadata response. Reader
and commenter links cannot fetch source snapshots or join source
synchronization. They read the current projection and receive rendered
annotations without private source anchors.

## Annotation HTTP results

POST /api/documents/{slug}/comments accepts these JSON messages. Include a
unique `request_id` for correlation. For comment and reply creation, also
include a UUID-shaped `temp_id`, kept unchanged across retries.

| `type` | Required action fields | Optional action fields |
|---|---|---|
| `comment` | `body`, `exact` (the selected words), `digest` for rendered annotations | `motivation` (defaults to commenting), `prefix`, `suffix`, `position`, `point`, `document`, `color` |
| `reply` | `comment_id`, `body` | -- |
| `resolve` | `comment_id`, `resolved` (boolean; false reopens) | -- |
| `delete` | `comment_id` | -- |
| `refine` | `comment_id`, `proposed`, `expected_proposed`, `revision` | `body` |
| `accept` | `comment_id` | -- |
| `reject` | `comment_id` | -- |

Readers and commenters connect for annotation events only. They must not send
`doc-open` or source updates; the server refuses source synchronization and
sends source-bearing updates only to editor peers. Source suggestions and
source selectors are private to editors.

Rendered comments carry the tree digest visible during selection, not a
bundle ID: there is no rendered bundle, so the projection's own tree digest
is the identity a rendered comment is checked against. An empty or obsolete
digest is refused for commenters. The head digest changing while a comment is
submitted is serialized with the annotation commit; a refusal keeps the
client submission available for recovery. Editor source annotations can omit
the digest.

`source-changed` carries only `digest`. It announces that the head digest has
moved; clients offer Refresh without replacing the visible page or discarding
drafts. It is coalesced to at most one per document per second and is sent to
readers and commenters, computed only when a warm cache exists -- otherwise
it is sent on the next flush.

A client sends what it saw: `exact` and, around it, `prefix` and `suffix`,
with `position` as the offset in the rendered text. It never sends a file, an
offset into one, or a version vector. What passage of what source file those
words are is worked out by the server, against the head it holds, at the
moment the comment is made -- and once, never again. A reader of a projected
rendering has no source to answer that question from, and an editor's answer
would have to be checked against the head anyway, so there is one answer and
the server gives it.

Words that appear in several places, with nothing in the surrounding text to
say which was meant, are refused (`type: error`) rather than attached to the
first of them. Selecting a longer passage resolves it.

Set `document: true` for a remark about the document as a whole. It takes no
selection and can never be orphaned.

A comment against the draft is recorded against the head as it stood when the
sequencer evaluated the command, which the server takes for it. A co-editor
typing between that evaluation and the write invalidates the moment rather
than the comment: the comment and the source operation it may depend on
commit in one transaction, so nothing interleaves between evaluation and
commit for that document (`SPEC-server-is-a-log.md` §7). Only a room edited
continuously enough to lose that race three times running answers `type:
error` with `"code": "stale_selection"`, which a client may resend.

The server supplies the author, timestamps, and the version vector, frontier
and tree digest the comment is anchored against; a submitted `creator` does
not override attribution. Highlighting may omit `body`. Text limits and
allowed motivations are deployment settings.

A stored comment carries three things a client can read:

- `original_anchor`: `{source_sequence, frontier, kind, target}`, what the
  comment is about. `kind` is `source_text` -- whose `target` is `{file_id,
  start_utf16, end_utf16, start_side, end_side, exact, prefix, suffix}` --
  or `document`. It is written once and no later event changes it. Editors
  only: a rendered reader is never sent source identities or source
  quotations.

  Two names for the state, because they answer different questions and one
  outlives the other (§8.2). `source_sequence` is the `document_updates`
  row that made the quoted text durable, which is what says the comment and
  the text it quotes were written in one transaction (§7 step 4).
  `frontier` is what `fork_at` reconstructs that state from, and unlike the
  row it names it survives the compaction that deletes it.
- `attachment`: `{tree_digest, status, resolved_range_utf16, diagnostic}`,
  where that passage is now, keyed by the projection digest it was computed
  against (§2.2). `status` is `exact`, `modified`, `ambiguous`,
  `deleted` or `unresolved`. It is a cache: the server recomputes it from the
  log whenever the document changes, and broadcasts one
  `{"type": "attachments", "attachments": [{comment_id, attachment}]}` event
  per pass to editor peers -- a single edit moves every comment after it, so
  the pass is the frame rather than the comment. Editors only.
- `presentation`: `{rendered_exact, rendered_prefix, rendered_suffix,
  rendered_position_utf16}`, the words as the page had them. Display evidence,
  sent to everyone who can see the comment, and never resolved through.

Every annotation is about words somebody selected. Two gestures that were not
have been withdrawn. A rectangle over a rendered figure names no range of any
source file, and naming what it is about needs provenance the renderers do not
emit. A note at a point between two words named one, but cost a second way to
locate a range and a second way to follow one through later edits -- a
parallel implementation of the hardest part of this protocol, for a remark
that selecting the neighbouring words makes just as well.

`position` remains. It is the offset in the rendered text: display evidence,
and a tie-breaker of last resort between passages that read alike. It is never
a place of its own.

`color`, when present, must be a six-digit `#RRGGBB` value and is retained for
commenting or highlighting annotations.

The result is the created or changed event:

    {
      "type": "comment",
      "request_id": "request-123",
      "version": 1,
      "protocol": "librepaper.room.v2",
      "comment": {}
    }

Creation retries with the same UUID and author are idempotent while that
record remains present: source-producing comment operations write their
retry record as a `document_labels` row keyed by the client's `request_id`,
not as a per-keystroke receipt (there is no receipt table, and none for
comments that do not touch source). Arbitrary non-UUID temporary labels do
not enable deduplication. A correlated retry or an unchanged resolve returns
noop: true; an ordinary browser retry keeps the legacy event shape. Errors
use the same request_id, version, and protocol fields and have type: error
plus a human-readable message. A response with HTTP success means the
annotation is durable.

`refine` replaces the text of one pending suggestion in place. Its ID, its
anchor and its pass remain unchanged, and `revision` must match the head the
suggestion was made against. Only its author or an editor may refine it. A
mismatched `expected_proposed` or `revision`, a decided suggestion, or an
acceptance still pending is refused. The resulting `refine` event carries
`comment_id` and the full updated `comment`. Retrying an already applied
replacement and note returns a no-op. Refining a proposal never changes the
document source.

`accept` and `reject` decide a pending suggestion and need an editor.
Accepting applies the proposal to the live source in the same fenced
transaction that admits the resulting log row, and answers with
`resolved_in`: the `source_sequence` of that row, which is the only durable
name the state has now that there are no version ids. When the passage has
changed since the suggestion was made the answer is `type: error` with
`stale: true` and nothing is applied. Rejecting resolves without applying. A
retry with the same `request_id` finds the `document_labels` row the first
attempt wrote and returns its result without doing the work again (§7.2).

## Labels

Send the `doc-label` message on `/ws/{slug}`:

    {"type": "doc-label", "why": "sync", "request_id": "label-123"}

An editor receives either a durable label:

    {
      "type": "doc-label",
      "sha": "label-tree-sha",
      "request_id": "label-123",
      "durable": true,
      "version": 1,
      "protocol": "librepaper.room.v2"
    }

or a durable no-op with noop: true when the live tree already has that
digest. Storage failures are type: error responses. Automation label
requests bypass the browser debounce window so a successful result means the
label has landed durably.

A label is recorded as a `document_labels` row: no precondition is
checked, and the row records the head vector, frontier and tree digest at the
moment it runs (`SPEC-server-is-a-log.md` §7.1, §8.2). `request_id`
correlates label results; it is not a stored replay key by itself, but a
retry with the same `request_id` finds the label the earlier attempt wrote
and returns it rather than writing a second one. An unchanged tree is a
no-op. Delete retries for an already absent annotation report an
unknown-comment error; confirm absence with a fresh snapshot when recovering
from a lost response.

## WebSocket messages

Automation peers connect to /ws/{slug} with the same key header and automation
marker. The document messages carry the synchronization transport described
in `SPEC-server-is-a-log.md` §6.

- `doc-open {protocol: "librepaper.room.v2", vector}` sends a base64-encoded
  Loro version vector. The protocol string is required and checked before any
  update is accepted; there is no `after` field, and there is no `doc-sync`
  message -- `doc-open` is the only way to join. A client that cannot present
  `librepaper.room.v2` receives upgrade-required before it can send an
  update, and its local state is left untouched.

  The server answers with one of, depending on how much of the client's
  vector the log already covers (`SPEC-server-is-a-log.md` §6.2):

  - `doc-state {vector, updates}` when the base and every row are covered:
    `updates` is the buffer's batches and `vector` is the head vector.
  - `doc-rows {vector, updates}` when the base is covered but some rows are
    not: `updates` is those rows' batches followed by the buffer's.
  - `doc-state {vector, base, updates}` when the base itself is not covered,
    or `doc-state {vector, ref, digest, updates}` above the inline size
    limit, where `ref` is a same-origin URL fetched with the document
    credentials and `digest` is checked against what arrives.

  `updates` is an ARRAY of base64 Loro blobs, applied with `importBatch`,
  and this is not a stylistic choice. Two Loro updates concatenated are not
  one Loro update: each blob carries its own checksum over its own bytes, so
  a decoder handed the concatenation reports "Checksum mismatch. The data is
  corrupted." A single `update` field would therefore have to be a single
  export, which would mean the server building a document to produce it --
  exactly what §1 says it does not do. `base`, when present, is one blob and
  is imported on its own before the array.

  No cache is built to answer a join; the server reads stored row vectors
  backwards from the head to decide which of these three replies applies.
  Because rows are causally complete and Loro discards operations it already
  holds, over-sending rows the client partly had is harmless. The client
  imports everything it receives, then exports `Updates { from: vector }`
  using the vector from the reply and sends that as one `doc-update` batch.
  This is today's catch-up; its acknowledgement empties the client's
  unacknowledged map, which together with having joined means the session is
  synced. An empty map alone means nothing until both are true.

- `doc-update {seq, update}` sends a base64-encoded Loro `update` and an
  increasing `seq` scoped to the socket. An update too large for one frame is
  split across `doc-update-start`, `doc-update-chunk` and `doc-update-end`
  under one `seq`. Incoming `doc-update` frames relayed from other peers must
  also be applied locally.

  The server decodes the update's header, not its content. A well-formed
  update whose start vector the head vector does not cover is a causal gap:
  the server replies `doc-gap {vector: head}` and drops the batch without
  appending it. The client exports `Updates { from: vector }` using the given
  vector and sends that instead. A malformed update -- one that fails the
  header checksum -- closes the socket with `invalid_update`; the client
  reconnects and reconciles through `doc-open`. Because every accepted batch
  is causally complete relative to what came before it, a gap is refused at
  the door rather than accepted and repaired later.

- `doc-ack {upTo}` acknowledges a flush: `upTo` is the highest `client_seq`,
  among the peer's own batches, in the row that was just made durable.
  `coverage` no longer exists; the client cannot ask what the server holds
  beyond what `doc-ack` and `doc-gap` already tell it. Acknowledgements
  always cover a contiguous prefix of a peer's session, because a batch is
  either refused before it is appended (a gap) or accepted, never both for
  the same batch.

- `doc-gap {vector}` is sent to an editor whose `doc-update` started before
  the vector the server's log currently covers. See above.

- `doc-label` receives the label result shapes above.
- comment, reply, resolve, delete, anchor, and refine receive the annotation
  result shapes above.
- A change awaiting an accept or reject is a proposal branch:
  `proposal-open`, `proposal-update`, `proposal-decide` and `proposal-list`
  are answered by `proposal-opened`, `proposal-updated`, `proposal-decided`
  and `proposal-list` (which carries a `proposals` array).
- `source-changed {digest}` is sent to readers and commenters, not editors,
  when the head digest changes. See "Annotation HTTP results" above.

Every requested room operation must carry a request_id when the caller needs
correlation. Errors are explicit and are never silently dropped for an
unauthorized annotation, edit, or label request. A missing or empty
request_id remains accepted for compatibility with browser clients.

The server bounds WebSocket frames and CRDT updates using the configured
message, document, file, and update-rate ceilings. A peer must reconnect after
transport closure and resynchronize with `doc-open` and a state vector. It
must retry annotations and labels with the same request and temporary
identifiers; it must not assume that receiving a relay means an update is
durable -- only `doc-ack` means that.

### Join algorithm

Restated precisely, because it is the part most worth getting right
(`SPEC-server-is-a-log.md` §6.2). Joining is handled as one message to the
document's sequencer task, so no batch can arrive and be relayed between the
server computing its `doc-open` reply and the client actually being
registered as a subscriber:

1. Register the subscriber first.
2. Find the earliest row whose stored vector is not covered by the client's
   vector, scanning stored vectors backwards from the head. This step and
   the one before it never need to decode or hold a `LoroDoc`.
3. If every row and the base are covered, reply `doc-state` with the current
   head vector and the buffer's batches as the update.
4. If some rows are uncovered but the base is covered, reply `doc-rows` with
   those rows plus the buffer.
5. Otherwise reply `doc-state` with the base by reference, followed by
   `doc-rows` for all rows plus the buffer.
6. The client imports everything, then exports and sends `Updates { from:
   vector }` from the reply, and that batch's acknowledgement is what empties
   the unacknowledged map.

### Crash contract

Stated exactly, because it is a narrower guarantee than "your work is safe"
(`SPEC-server-is-a-log.md` §6.4). Between a flush and the next one, an
editor's typing lives only in clients and the sequencer's in-memory buffer.
If the server process dies with a non-empty buffer:

- Every batch in that buffer is still in its author's own Loro document,
  because relay happens before flush and the author always has their own
  work locally.
- On reconnect, that author's catch-up export (the `doc-open` handshake
  above) contains those operations, and they become durable on the next
  flush after that.
- Other editors who received the batch by relay before the crash hold it
  too, and can supply it on their own reconnect even if the original author
  never comes back.
- A reader's projection lacks that work until a flush happens, because
  readers never hold CRDT bytes.

**Unacknowledged work is recoverable if a surviving client retained it and
reconnects. That is the guarantee, stated exactly.** It is not a guarantee
that unacknowledged work survives no matter what: if every client that held
a given batch is gone before it is flushed, that batch is gone. Local
persistence in each client (IndexedDB, unchanged in mechanism) is what makes
"reconnects" possible after a browser restart; it is the outbox, not the
server's buffer.

## Compatibility

Unknown response fields must be ignored. Clients must require
protocol == librepaper.room.v2 (or the snapshot protocol) only when they need
the v2 guarantees, and otherwise retain the pre-v2 browser behavior. The
server accepts messages without request_id so existing browser bundles can
continue to connect during a rolling deployment. There is no compatibility
path for `doc-sync` or for a `doc-open` that omits `protocol`: those receive
upgrade-required, because `doc-gap` and the vector-only acknowledgement model
depend on the server and client agreeing on causal-gap handling from the
first frame.
