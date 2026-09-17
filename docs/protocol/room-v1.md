# Existing LibrePaper room operations for automation

This document describes the HTTP and WebSocket contract used by headless
automation peers. The credential is the value of the x-librepaper-key header.
Clients may also send x-librepaper-automation: 1; this is required for
link-bounded operation when a machine also has a cached signed-in browser
session. The server keeps that identity for attribution and policy ceilings,
but the live link remains the complete document authority. Signed-in comments
retain the account's normal attribution; an anonymous link-only peer receives
a stable link pseudonym.

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

source, tree, files, texts, and comments come from the same room lock.
source_sha is the SHA-256 digest of the returned source bytes. The top-level
main is the live tree's main path; clients must not use a stale main path from
an older document metadata response. Reader and commenter links cannot fetch
source snapshots or join source synchronization. They read the current published
HTML and receive rendered annotations without private source anchors.

## Annotation HTTP results

POST /api/documents/{slug}/comments accepts these JSON messages. Include a
unique `request_id` for correlation. For comment and reply creation, also
include a UUID-shaped `temp_id`, kept unchanged across retries.

| `type` | Required action fields | Optional action fields |
|---|---|---|
| `comment` | `body`, `exact` (the selected words), `publication_id` for rendered annotations | `motivation` (defaults to commenting), `prefix`, `suffix`, `position`, `point`, `document`, `color` |
| `reply` | `comment_id`, `body` | — |
| `resolve` | `comment_id`, `resolved` (boolean; false reopens) | — |
| `delete` | `comment_id` | — |
| `refine` | `comment_id`, `proposed`, `expected_proposed`, `revision` | `body` |
| `accept` | `comment_id` | — |
| `reject` | `comment_id` | — |

Readers and commenters connect for annotation events only. They must not send
`doc-open`, `doc-sync`, or source updates; the server refuses source synchronization
and sends source-bearing updates only to editor peers. Source suggestions and
source selectors are private to editors.

Rendered comments carry the publication ID visible during selection. An empty
or obsolete ID is refused for commenters. A publication changing while a comment
is submitted is serialized with the annotation commit; a refusal keeps the client
submission available for recovery. Editor source annotations can omit the ID.

`publication-updated` carries only `publication_id`. It announces availability;
clients offer Refresh without replacing the visible page or discarding drafts.

A client sends what it saw: `exact` and, around it, `prefix` and `suffix`,
with `position` as the offset in the rendered text. It never sends a file, an
offset into one, or a checkpoint. What passage of what source file those words
are is worked out by the server, against the checkpoint it holds, at the moment
the comment is made -- and once, never again. A reader of a published rendering
has no source to answer that question from, and an editor's answer would have
to be checked against the checkpoint anyway, so there is one answer and the
server gives it.

Words that appear in several places, with nothing in the surrounding text to
say which was meant, are refused (`type: error`) rather than attached to the
first of them. Selecting a longer passage resolves it.

Set `document: true` for a remark about the document as a whole. It takes no
selection and can never be orphaned.

The server supplies the author, timestamps, and the checkpoint the comment is
anchored against; a submitted `creator` does not override attribution.
Highlighting may omit `body`. Text limits and allowed motivations are
deployment settings.

A stored comment carries three things a client can read:

- `original_anchor`: `{checkpoint_id, kind, target}`, what the comment is
  about. `kind` is `source_text` -- whose `target` is `{file_id, start_utf16,
  end_utf16, start_side, end_side, exact, prefix, suffix}` -- or `document`.
  It is written once and no later event changes it. Editors only: a rendered
  reader is never sent source identities or source quotations.
- `attachment`: `{checkpoint_id, status, resolved_range_utf16, diagnostic}`,
  where that passage is now. `status` is `exact`, `modified`, `ambiguous`,
  `deleted` or `unresolved`. It is a cache: the server recomputes it from the
  document's own history whenever the document changes, and broadcasts an
  `attachment` event to editor peers. Editors only.
- `presentation`: `{rendered_exact, rendered_prefix, rendered_suffix,
  rendered_position_utf16}`, the words as the page had them. Display evidence,
  sent to everyone who can see the comment, and never resolved through.

Figure-region annotations are not part of this version. A rectangle over a
rendered image names no range of any source file, and naming what it is about
needs provenance the renderers do not emit.

Set `point: true` for a comment bubble at a single rendered-text position. A
point comment must use `motivation: "commenting"`, an empty `exact`, and a
nonnegative `position`; it still requires a nonempty body. It is anchored to
the place between the words on either side of it, which is what `prefix` and
`suffix` are for. `color`, when
present, must be a six-digit `#RRGGBB` value and is retained for commenting or
highlighting annotations. `point: false` is omitted from wire responses.

The result is the created or changed event:

    {
      "type": "comment",
      "request_id": "request-123",
      "version": 1,
      "protocol": "librepaper.room.v1",
      "comment": {}
    }

Creation retries with the same UUID and author are idempotent while that
record remains present. Arbitrary non-UUID temporary labels do not enable
deduplication. A correlated retry or
an unchanged resolve returns noop: true; an ordinary browser retry keeps the
legacy event shape. Errors use the same request_id, version, and protocol
fields and have type: error plus a human-readable message. A response with
HTTP success means the annotation is durable.

`refine` replaces the text of one pending suggestion in place. Its ID, its
anchor and its pass remain unchanged, and `revision` must match the checkpoint
the suggestion was made against. Only its author or an
editor may refine it. A mismatched `expected_proposed` or `revision`, a decided
suggestion, or an acceptance still pending is refused. The resulting `refine`
event carries `comment_id` and the full updated `comment`. Retrying an already
applied replacement and note returns a no-op. Refining a proposal never changes
the document source.

`accept` and `reject` decide a pending suggestion and need an editor. Accepting
applies the proposal to the live source, takes a checkpoint, and answers with
`resolved_in`; when the passage has changed since the suggestion was made the
answer is `type: error` with `stale: true` and nothing is applied. Rejecting
resolves without applying. A retry with the same `request_id` returns the
recorded outcome with `noop: true`.

## Checkpoints

Send the `doc-checkpoint` message on `/ws/{slug}`:

    {"type": "doc-checkpoint", "why": "sync", "request_id": "checkpoint-123"}

An editor receives either a durable checkpoint:

    {
      "type": "doc-checkpoint",
      "sha": "checkpoint-tree-sha",
      "request_id": "checkpoint-123",
      "durable": true,
      "version": 1,
      "protocol": "librepaper.room.v1"
    }

or a durable no-op with noop: true when the live tree already has that
checkpoint. Storage failures are type: error responses. Automation checkpoint
requests bypass the browser debounce window so a successful result means the
checkpoint has landed durably.

`request_id` correlates checkpoint results; it is not a stored replay key.
A retry checkpoints the current source and may include edits that arrived
after the first request. An unchanged tree is a no-op. Delete retries for an
already absent annotation report an unknown-comment error; confirm absence
with a fresh snapshot when recovering from a lost response.

## WebSocket messages

Automation peers connect to /ws/{slug} with the same key header and automation
marker. The document messages carry the synchronization transport:

- `doc-open` or `doc-sync` sends a base64-encoded Loro version `vector` and
  receives `doc-state` with `update`, `vector`, and peer `count`. Large state
  is returned as `ref` instead of `update`: fetch that same-origin URL with the document
  credentials, apply its bytes, then send `doc-sync` to catch up.
- `doc-update` sends a base64-encoded Loro `update` and an increasing `seq`
  scoped to the socket. It receives `doc-ack` with the durable `seq` only after
  persistence. An update too large for one frame is split across
  `doc-update-start`, `doc-update-chunk` and `doc-update-end` under one `seq`.
  Acknowledgements through a sequence cover earlier updates on that socket. Incoming `doc-update` frames from other peers must also be
  applied locally.
- `doc-checkpoint` receives the checkpoint result shapes above.
- comment, reply, resolve, delete, anchor, and refine receive the annotation
  result shapes above.
- A change awaiting an accept or reject is a proposal branch:
  `proposal-open`, `proposal-update`, `proposal-suggest`, `proposal-list` and
  `proposal-decide` are answered by `proposal-opened`, `proposal-updated`,
  `proposals` and `proposal-decided`. A decision names a `hunk`; the room
  document moves only when the proposal resolves.

Every requested room operation must carry a request_id when the caller needs
correlation. Errors are explicit and are never silently dropped for an
unauthorized annotation, edit, or checkpoint request. A missing or empty
request_id remains accepted for compatibility with browser clients.

The server bounds WebSocket frames and CRDT updates using the configured
message, document, file, and update-rate ceilings. A peer must reconnect after
transport closure and resynchronize with a state vector. It must retry
annotations and checkpoints with the same request and temporary identifiers;
it must not assume that receiving a relay means an edit is durable.

## Compatibility

Unknown response fields must be ignored. Clients must require
protocol == librepaper.room.v1 (or the snapshot protocol) only when they need
the v1 guarantees, and otherwise retain the pre-v1 browser behavior. The
server accepts messages without request_id so existing browser bundles can
continue to connect during a rolling deployment.
