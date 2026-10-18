# Room HTTP and WebSocket protocol

This document describes current automation and browser room operations.
Source synchronization uses `librepaper.room.v3`; snapshots use
`librepaper.snapshot.v1`; annotation reads use
[`librepaper.comments.v1`](comments-v1.md).

Automation sends its document credential in `X-LibrePaper-Key` and may send
`X-LibrePaper-Automation: 1`. Link-bounded requests must include the
automation marker when a browser session is also present. Document access is
rechecked for socket operations. Signed-in comments use account attribution;
anonymous link users receive a stable link pseudonym.

## Snapshot and annotation reads

- `GET /api/documents/{slug}/snapshot` requires editor or owner access and
  returns source, tree, files, texts, role, and capabilities from one
  room-locked projection, plus the first comment page and `comment_state`.
- Snapshot protocol is `librepaper.snapshot.v1`. `comments` is bounded; use
  `GET /api/documents/{slug}/comments?cursor=&limit=` and
  `GET /api/documents/{slug}/comments/{comment_id}/replies?cursor=&limit=`
  to continue. Reader and commenter links cannot fetch source snapshots or
  join source synchronization.
- See [comments-v1.md](comments-v1.md) for page bounds, cursor bindings,
  consistency, and room events.

## Annotation writes

`POST /api/documents/{slug}/comments` accepts these action types. Include a
`request_id` when the caller needs correlation; comment/reply creation also
uses a stable UUID `temp_id` across retries.

| Type | Required fields | Notes |
|---|---|---|
| `comment` | `exact` for a selected passage | `body` may be omitted for a highlight. Optional `motivation`, `prefix`, `suffix`, rendered `position`, `document`, `color`, and rendered digest. `document: true` is a document-wide remark and has no selection. |
| `reply` | `comment_id`, `body` | Adds a reply to a thread. |
| `resolve` | `comment_id`, `resolved` | `false` reopens. |
| `delete` | `comment_id` | Deletes an annotation. |

The server supplies author, timestamps, and the source version used for the
anchor. Clients submit the selected rendered words and surrounding context;
the server resolves them against the current head. Ambiguous text is refused
instead of attached to an arbitrary match. Use `document: true` for a
document-wide remark. Rendered annotations require the current tree digest;
editor source annotations may omit it. Comment color, when supplied, is a
six-digit `#RRGGBB` value.

Stored comments distinguish the immutable `original_anchor`, current cached
`attachment`, and rendered `presentation`. Source identities and quotations
are editor-only; readers receive rendered presentation. Attachment status is
one of `exact`, `modified`, `ambiguous`, `deleted`, or `unresolved`.

- A stale selection can return `type: "error"` with `code: "stale_selection"`;
  clients may retry while preserving the submission.
- Creation and decision retries use stored request identities where
  supported. A repeated operation returns its recorded result or a no-op;
  a lost response can also be reconciled through the agent operation receipt
  APIs. HTTP success means the annotation operation committed.
- `source-changed {digest}` notifies readers and commenters that the current
  tree digest moved. It does not replace the visible page or discard drafts.

Text limits and allowed motivations are deployment settings.

## Labels

An editor sends `{"type":"doc-label","why":"sync","request_id":"..."}`
on `/ws/{slug}`. The response includes `sha`, `request_id`, `durable: true`,
`noop`, and protocol/version fields. A label records the current head vector,
frontier, and tree digest; it does not change source. A new label request
always records a row, even if the tree is unchanged. When `request_id` is a
UUID, it is the replay key: retrying it returns the previously recorded row.
`noop` compares the returned digest with the latest label read before the
request. The browser may debounce its own label requests; automation requests
are processed immediately. Authorization or storage failures return an
explicit error.

## Room WebSocket

Peers connect to `/ws/{slug}` with document credentials. Annotation `hello`
and `comments-changed` frames are described in [comments-v1.md](comments-v1.md).
Readers and commenters receive annotations and `source-changed`, but cannot
send or receive source CRDT updates. Source synchronization is editor-only.

### Source synchronization

- An editor must first send
  `{"type":"doc-open","protocol":"librepaper.room.v3","vector":"..."}`.
  There is no `doc-sync` or `after` form. The server registers the editor and
  computes catch-up in one sequencer operation before replying.
- Catch-up replies include base64 vectors: `vector` is the current head,
  including buffered work; `durableVector` is committed work only. The server
  sends one of:
  - `doc-state {vector, durableVector, updates}` when the client already has
    the compacted base and stored rows;
  - `doc-rows {vector, durableVector, updates}` when stored rows remain;
  - `doc-state` with `base` (or `ref` and `digest` for a large baseline),
    followed by `doc-rows` when the client lacks the compacted base.
- `updates` is an array of independently encoded Loro batches; the client
  applies it with `importBatch`. `base`, when present, is one independent blob
  imported before the array. After import, the client exports
  `Updates { from: vector }` using the reply's head vector and sends it as
  `doc-update`.
- `doc-update {seq, update}` carries one encoded update and a socket-scoped
  increasing sequence. Oversized updates use `doc-update-start`, indexed
  `doc-update-chunk` frames, and `doc-update-end` with the same sequence.
  Relayed updates from other editors must also be applied locally.
- The server checks the update header and causal start vector. An uncovered
  dependency returns `doc-gap {vector}` and drops the batch; the client
  exports again from that vector. A malformed update closes the socket with
  `invalid_update`.
- `doc-ack {upTo}` retires transmission bookkeeping after a durable flush. An
  empty update may be acknowledged without a stored row. Neither case proves
  earlier work durable.
- `doc-durable {vector}` is the committed log vector, broadcast after flushes
  and semantic command commits. The browser captures `saveTarget` after each
  local edit, including restored local work; remote imports do not advance it.
  Merge durable vectors monotonically, and consider local work remotely saved
  when durable coverage includes `saveTarget`. Cancel an armed pressure retry
  when that happens. Local persistence and remote durability are separate; a
  relay or transport acknowledgement is not a durability signal.
- Update refusals use `error {seq, reason, retryable, vector, message}`.
  `reason` is the stable code; `message` is explanatory. Retryable refusals
  include the head `vector` and the refused update was not merged. Clients
  retry from that vector with backoff. The browser starts at one second and
  doubles to thirty seconds, with one retry outstanding. The server closes
  after three consecutive client-attributed refusals; pressure refusals are
  exempt.

  | Reason | Retryable | Server behavior |
  |---|---|---|
  | `rate` | yes | Principal update allowance exceeded; counts toward socket closure. |
  | `log_quota` | yes | Document log awaits compaction; counts toward socket closure. |
  | `document_buffer` | yes | Unsaved document buffer is full; pressure exemption. |
  | `pending_budget` | yes | Deployment pending-work budget is full; pressure exemption. |
  | `refused` | no | Non-retryable ceiling; counts toward socket closure. |

Other room frames include `doc-label`, annotation action results, and
`source-changed`. Request IDs correlate operations; clients
that need correlation should send them. Unauthorized operations receive an
explicit error.

### Tracked changes

Tracked changes are ordinary document updates. They have no messages of their own. Accepting or rejecting one is a normal document edit that syncs through the same update path. See [track changes](../../architecture/review.html#track-changes).

## Project chat

Project chat is stored per project. Only commenters and above receive it.
View-only links never see chat, in `hello`, in broadcasts or over HTTP.
Chat never enters the document log. The server buffers messages per room
and writes them in batches, one transaction every 2 seconds or every 64
messages. Chat counts toward the owner's storage, with no message cap.

A message object has this shape:

```json
{"type":"chat","id":"<uuid>","text":"...","creator":"<label>","created":"<timestamp>"}
```

`id` is a UUIDv7 that the server makes at send time. It orders messages. The
sender's own echo also carries `temp_id`.

For commenters and above, `hello` gains three fields. A viewer's `hello`
has none of them.

- `chat`: the newest 50 messages, oldest first.
- `chat_more`: `true` when older messages exist.
- `chat_can_clear`: `true` only for the project owner.
- `chat_unavailable`: `true` instead of `chat` and `chat_more` when the server could not read the history. The client keeps its list and fetches the newest page again.

Every reconnect gets a fresh `hello`, so the `hello` fills in missed
messages. Clients merge messages by `id`.

Older messages load over HTTP. `GET /api/documents/{slug}/chat-history?before=<uuid>&limit=<n>`
requires commenter access or above. `limit` defaults to 50 and is capped at 100.
It returns `{"messages":[...],"more":bool}`, with the messages oldest first.

Clearing chat is owner-only. The client sends `{"type":"chat-clear"}`.

- If the sender is the owner, the server deletes every message. It then
  broadcasts `{"type":"chat-cleared"}` to every commenter-or-above socket,
  including the sender.
- Otherwise the server replies with `{"type":"error","message":"only the owner can clear chat"}`.

Only a history trim by the owner deletes chat and broadcasts `{"type":"chat-cleared"}`. An editor's trim leaves chat alone.

When the owner's storage is full, a send gets
`{"type":"error","message":"the project owner's storage is full","temp_id":...}`.
The message is not stored.

## Recovery and compatibility

The server's unsaved buffer is in memory. If the process stops before flush,
unflushed work is recoverable only from a surviving editor that retained it
and reconnects; a reader's projection does not contain buffered CRDT bytes.
Clients keep local state, reconcile with `doc-open`, and use `doc-durable` to
confirm storage coverage.

Unknown response fields should be ignored. Source synchronization requires
`librepaper.room.v3`; clients that do not send it are closed with
`upgrade_required` before updates are accepted. The snapshot and comment
protocol versions are independent. Comments are paged under
`librepaper.comments.v1`; there is no whole-collection compatibility shape.

Implementation: [server/socket.rs](../../../crates/librepaper-server/src/server/socket.rs),
[room/message.rs](../../../crates/librepaper-room/src/room/message.rs),
[room/label.rs](../../../crates/librepaper-room/src/room/label.rs),
[automation/peer.rs](../../../crates/librepaper-companion/src/automation/peer.rs),
[web/src/lib/room.js](../../../web/src/lib/room.js).
