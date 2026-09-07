# Existing Komodoc room operations for automation

This document describes the HTTP and WebSocket contract used by headless
automation peers. The credential is the value of the x-komodoc-key header.
Clients may also send x-komodoc-automation: 1; this is required for
link-bounded operation when a machine also has a cached signed-in browser
session. The server keeps that identity for attribution and policy ceilings,
but the live link remains the complete document authority. Signed-in comments
retain the account's normal attribution; an anonymous link-only peer receives
a stable link pseudonym.

## Snapshot

GET /api/documents/{slug}/snapshot returns one room-locked read:

    {
      "version": 1,
      "protocol": "komodoc.snapshot.v1",
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
      "role": "commenter",
      "capabilities": {"read": true, "comment": true, "edit": false}
    }

source, tree, files, texts, and comments come from the same room lock.
source_sha is the SHA-256 digest of the returned source bytes. The top-level
main is the live tree's main path; clients must not use a stale main path from
an older document metadata response. A reader link may fetch a snapshot. A
commenter or editor link receives the corresponding capabilities.

## Annotation HTTP results

POST /api/documents/{slug}/comments accepts these JSON messages. Include a
unique `request_id` for correlation. For comment and reply creation, also
include a UUID-shaped `temp_id`, kept unchanged across retries.

| `type` | Required action fields | Optional action fields |
|---|---|---|
| `comment` | `body`, `exact` (the selected words) | `motivation` (defaults to commenting), `prefix`, `suffix`, `position`, `source` |
| `reply` | `comment_id`, `body` | — |
| `resolve` | `comment_id`, `resolved` (boolean; false reopens) | — |
| `delete` | `comment_id` | — |
| `anchor` | `comment_id`, `source` | — |

`source` is `{path, exact, prefix, suffix, position}`: the file path and
selected source text, with optional surrounding text and a nonnegative
position hint. It identifies the source passage separately from `exact`,
which identifies the passage in the rendered document. The server supplies
the author, timestamps, and checkpoint references; a submitted `creator`
does not override attribution. Highlighting may omit `body`. A figure region
can replace the text selection, as supported by the browser's annotation
interface. Text limits and allowed motivations are deployment settings.

The result is the created or changed event:

    {
      "type": "comment",
      "request_id": "request-123",
      "version": 1,
      "protocol": "komodoc.room.v1",
      "comment": {}
    }

Creation retries with the same UUID and author are idempotent while that
record remains present. Arbitrary non-UUID temporary labels do not enable
deduplication. A correlated retry or
an unchanged resolve returns noop: true; an ordinary browser retry keeps the
legacy event shape. Errors use the same request_id, version, and protocol
fields and have type: error plus a human-readable message. A response with
HTTP success means the annotation is durable.

## Checkpoints

Send the existing `y-checkpoint` message on `/ws/{slug}`:

    {"type": "y-checkpoint", "why": "sync", "request_id": "checkpoint-123"}

An editor receives either a durable checkpoint:

    {
      "type": "y-checkpoint",
      "sha": "checkpoint-tree-sha",
      "request_id": "checkpoint-123",
      "durable": true,
      "version": 1,
      "protocol": "komodoc.room.v1"
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
marker. The existing Yjs messages remain the synchronization transport:

- `y-open` or `y-sync` sends a base64-encoded Yjs state `vector` and receives
  `y-state` with `update`, `vector`, and peer `count`. Large state is returned
  as `ref` instead of `update`: fetch that same-origin URL with the document
  credentials, apply its bytes, then send `y-sync` to catch up.
- `y-update` sends a base64-encoded Yjs v1 `update` and an increasing `seq`
  scoped to the socket. It receives `y-ack` with the durable `seq` only after
  persistence. Acknowledgements through a sequence cover earlier updates on
  that socket. Incoming `y-update` frames from other peers must also be
  applied locally.
- y-checkpoint receives the checkpoint result shapes above.
- comment, reply, resolve, delete, and anchor receive the annotation result
  shapes above.

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
protocol == komodoc.room.v1 (or the snapshot protocol) only when they need
the v1 guarantees, and otherwise retain the pre-v1 browser behavior. The
server accepts messages without request_id so existing browser bundles can
continue to connect during a rolling deployment.
