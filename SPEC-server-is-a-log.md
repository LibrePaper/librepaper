# The server is a log

*2026-09-19. Proposed implementation specification. Not a description of
completed work.*

This specification replaces the document-engine parts of
`SPEC-document-architecture.md`: the resident authoritative `LoroDoc`, candidate
preparation, per-batch validation and measurement, server-side repair, commit
before relay for typing, receipts for typing, and the coherent handshake by
version vector. It keeps that specification's single-writer lease and epoch,
fenced transactions for semantic commands, immutable document identity,
authorization at the commit boundary, the two-origin isolation model, Loro on
both sides, and rendering in the browser. It is idea 2.1 of
`REVIEW-BIG-IDEAS.md` written out, with the corrections from the review of that
idea folded in as obligations.

## 1. The rule

The server stores and forwards source bytes. It interprets them only on demand,
and whatever it computes from them is a cache that can be thrown away and
rebuilt from the log.

Three consequences follow, and every section below is one of them worked out.

1. Ordinary typing costs the server an append and a relay. No fork, no repair,
   no history export, no validation of document shape, no install.
2. Every document-shaped read (reader projection, comment anchoring, proposal
   evaluation, export, agent reads) is a pure function of the log up to a
   named sequence, computed through a bounded cache.
3. Semantic commands still run as authorized, fenced database transactions,
   and each one names the exact log position it was evaluated against.

The server remains authoritative over who may write, what was accepted, and
what is durable. It stops being authoritative over what the bytes mean.

## 2. Product contract

### 2.1 Preserved

- Typing is local and immediate. Collaborators see it within the relay
  latency, not the persistence latency.
- Readers and commenters never receive CRDT bytes or history. They receive a
  projection of committed source.
- A server acknowledgement means the bytes are in PostgreSQL.
- Comments, proposal decisions, labels and restores are atomic with any source
  they depend on.
- Historical states named by a label or a comment remain reconstructible.
- Plain source-and-asset export needs no CRDT decoder on the user's side.
- One writing process per deployment, fenced by lease and epoch.

### 2.2 Changed

- The server no longer rejects an update because of what it contains, only
  because of who sent it, how large it is, or how fast it arrived.
- Directory collisions are not repaired by the server. They are resolved by a
  deterministic display rule that every consumer applies identically.
- Between a relay and its flush, other editors may see text that PostgreSQL
  does not yet hold. If the process dies in that window, the text is
  recovered when the editor who typed it reconnects and resends. That editor's
  own screen keeps showing it throughout.
- A document whose projection exceeds resource bounds becomes explicitly
  unreadable on the server until an operator or export recovers it. Editors
  can still type and still export their local copy.

### 2.3 Dropped

- Server-side guarantee of document shape.
- Server-side path normalization as a mutation.
- Per-keystroke receipts and the "reused request id with different content is
  refused" rule.
- Durable Loro vector coverage in acknowledgements.
- The distributed job queue and its polling.
- Archives written eagerly at label time.

## 3. Vocabulary

| Term | Meaning |
|---|---|
| Batch | One Loro update as a client sent it, with the client's own `seq` |
| Row | One `document_updates` row: a framed list of batches flushed together |
| Sequence | The row's `update_sequence`, monotonic per document, assigned by the sequencer |
| Head | The document state after every row plus the unflushed buffer |
| Cut | The document state after rows up to a named sequence and nothing later |
| Projection | The text tree, main path, format, assets and metadata derived from a cut or from head |
| Cache | A `LoroDoc` held in memory that is at head; disposable |
| Sequencer | The per-document task that owns the buffer, the counter and command ordering |

## 4. Components

### 4.1 The sequencer

One Tokio task per resident document, created on first use by the registry and
dropped when idle. It owns, and nothing else touches:

- the next `update_sequence`, seeded from the log on load;
- the flush buffer: an ordered list of `(peer_key, client_seq, bytes)`;
- the buffer's age and byte size;
- the subscriber list with each subscriber's role and bounded outbound queue;
- a handle to the cache entry for this document;
- the last acknowledged `client_seq` per peer for this process lifetime.

It accepts these messages and handles them strictly in order:

| Message | Effect |
|---|---|
| `Ingest(peer, client_seq, bytes)` | Import into cache, append to buffer, relay to editor subscribers, maybe flush |
| `Flush(reason)` | Write one row, advance sequence, acknowledge the batches it held |
| `Semantic(command)` | Flush and commit the command in one transaction (section 7) |
| `Subscribe(role, queue)` and `Unsubscribe` | Membership |
| `Project(at)` | Return the projection at head or at a cut |
| `Evict` | Flush, then drop the cache and the task |

It is a real actor: state lives inside the task, callers send messages and
await replies. There is no lease and no shared mutex. Callers that need a
read take a `Project` reply, never a reference into the task.

### 4.2 The log

`document_updates` is the source of truth for source. A row is:

```
document_id      uuid
update_sequence  bigint      per document, from the sequencer
update_bytes     bytea       framed batches, section 4.2.1
frontier         bytea       Loro frontiers of head after this row
created_at       timestamptz
```

`document_bases` is unchanged: one full-history Loro snapshot per document,
replacing rows with sequence at or below `through_update_sequence`.

#### 4.2.1 Framing

`update_bytes` is `0x01`, then repeated `[peer_key_len u16][peer_key][client_seq u64][bytes_len u32][bytes]`.
The frame carries attribution and lets replay hand each batch to
`LoroDoc::import_batch`. A row is written by exactly one process and never
updated.

#### 4.2.2 The frontier column

The sequencer records `cache.oplog_frontiers()` after importing the row's
batches. It costs nothing because the cache is at head. It is what lets a
label, a comment or a restore name an exact state that survives compaction,
because the base is a full-history export and `fork_at(frontier)` works
against it.

### 4.3 The cache

A process-wide LRU of `LoroDoc` instances keyed by document, bounded by an
estimated byte total. An entry is built by loading the base, replaying rows
with `import_batch`, then importing the buffer. While a document has
subscribers its entry is pinned. When the last subscriber leaves the entry
stays until evicted by pressure.

The cache is the only place the server interprets Loro. Everything that needs
a document goes through it:

- `Project(head)` reads the cache directly.
- `Project(cut S)` with `S` at head is the same. For an older `S` it is
  `cache.fork_at(frontier_of(S))`, a temporary document discarded after use.
  The frontier comes from the row or from the label that named it.
- Server-authored source (section 7.3) is produced by forking the cache,
  applying, and exporting updates from the cache's version vector.

Rebuild bounds (section 9) apply to construction. A cache entry that fails to
build marks the document unreadable.

### 4.4 The projection rule

A projection is a pure function of a `LoroDoc`. It is implemented once in Rust
and once in JavaScript, and the two are held equal by shared fixtures checked
into the repository. The rule tolerates every shape a valid Loro document can
have:

- `files` entries whose value is not a text container are absent.
- `paths` entries whose key is not a `files` key are absent.
- A `files` key with no `paths` entry is absent from the tree and reported in
  diagnostics.
- Two ids that resolve to the same path after NFC and case folding keep the
  path for the id whose creation is causally later; every other id shows as
  `name (2).ext`, `name (3).ext` in id order. Nothing is written.
- `meta.main` naming an absent path resolves to the first text file in path
  order, or to none. Nothing is written.
- Unknown `meta` keys are ignored. An unknown schema version is refused with
  upgrade-required, not projected.

Fixtures include astral characters, combining marks, case and normalization
collisions, an empty project, a deleted main file, and a `files` value of the
wrong container kind.

### 4.5 Fan-out

Relay is a copy of the incoming frame to every editor subscriber except the
sender, through the bounded per-subscriber queue that already exists. A
subscriber whose queue overflows is closed and reconnects with `after`
(section 6). Readers and commenters receive a `source-changed {sequence}`
notification on flush, never bytes.

## 5. The typing path

For each `doc-update` (or reassembled chunked update):

1. Authorize: the socket's role is editor and the writer lease is held. Both
   are the existing checks.
2. Bound: the update is at most `MAX_UPDATE_BYTES` (4 MiB); the sender is
   within its per-principal rate; the document is within its log quota
   (section 9). Refusal is a retryable close with a reason.
3. Ingest: send `Ingest` to the sequencer. The sequencer calls
   `cache.import(bytes)`. A `LoroError` means the bytes are not a Loro update;
   the batch is dropped and the sender gets an error frame. An import that
   succeeds with pending spans (causal gap) is accepted; those operations stay
   invisible in projections until their dependencies arrive, which the
   sender's own resend guarantees.
4. Buffer and relay: append to the buffer, relay to editors.
5. Flush when any of these holds:
   - the buffer has been non-empty for `FLUSH_MAX_AGE` (30 s);
   - no batch has arrived for `FLUSH_QUIET` (5 s);
   - the buffer exceeds `FLUSH_MAX_BYTES` (1 MiB);
   - a semantic command arrives (section 7);
   - the last subscriber leaves, the task is evicted, or the process is
     shutting down.
6. Flush writes one transaction: the row, `documents.update_sequence`, and the
   compaction counters. On commit, the sequencer sends `doc-ack` to every
   peer whose batches were in the row.

Import into the cache costs microseconds. The append is one small insert. No
step's cost depends on the document's history length.

### 5.1 Cost

Rows per actively edited document per minute:

| Cadence | Rows per minute |
|---|---|
| Spec target, 500 ms commit before relay | 120 |
| Spec fallback, 2 s | 30 |
| This design, 30 s max age | 2 |

A deployment with ten people typing continuously writes about twenty small
rows a minute and nothing when they stop.

## 6. Acknowledgement and replay

The client already numbers its batches, holds them in an unacknowledged map,
and clears the map on `doc-ack {upTo}`. This design keeps that and replaces
vector coverage with the log sequence.

### 6.1 Frames

| Frame | Direction | Change |
|---|---|---|
| `doc-update {seq, update}` and the chunked triple | client to server | unchanged |
| `doc-ack {upTo, sequence}` | server to client | `sequence` is the row that holds batch `upTo`; `coverage` is removed |
| `doc-open {after?}` | client to server | `after` is the last `sequence` this client saw; `vector` is removed |
| `doc-state {sequence, update}` or `{sequence, ref, digest}` | server to client | as today, plus the head `sequence` |
| `doc-rows {from, to, update}` | server to client | rows after `after`, framed as one update batch |
| `doc-update {update}` | server to client | relay, unchanged |
| `source-changed {sequence}` | server to readers | replaces the editor-only relay for non-editors |

`doc-sync` is removed. A client that missed frames reconnects with `after`.

### 6.2 Join

1. Client sends `doc-open {after}` with the last sequence it persisted, or
   nothing for a fresh client.
2. The sequencer answers from the cache. With no `after`, it sends the full
   state (`export Snapshot` from the cache, by reference above the inline
   limit as today) and the head sequence. With `after`, it sends the rows
   after that sequence as one `doc-rows`, plus the buffer contents; if the
   rows are compacted away it falls back to full state.
3. Client imports, then resends everything still in its unacknowledged map
   with its original `seq` values.
4. Server ingests those like any batch. Loro discards operations it already
   holds, so a resend after an ack that was lost in flight is harmless.

Registration and baseline are one message handled by one task, so no row can
fall between the state the client received and the first relay it sees.

### 6.3 Durability facts the client keeps

- `unacknowledged`: batches not yet in a `doc-ack`. Persisted locally as
  today.
- `lastSequence`: the highest `sequence` from any `doc-ack`, `doc-state`,
  `doc-rows` or `source-changed`. Persisted locally; drives `after`.

"Synced" means `unacknowledged` is empty. Nothing else is inferred from the
connection state.

### 6.4 Crash contract

If the process dies with a non-empty buffer:

- every batch in it is still in the sending client's unacknowledged map, so
  it is resent on reconnect and becomes durable then;
- other editors imported it and keep it locally; when the sender's resend is
  flushed their copies are confirmed by the log;
- a reader's projection lacks it until that flush.

Unacknowledged work is recoverable if a surviving client retained it and
resubmits it. That is the guarantee, stated exactly.

## 7. Semantic commands

A semantic command is anything that writes relational state whose meaning
depends on the source: comment, reply, resolve, delete, proposal open, update
and decide, label, restore, agent patch, whole-project replacement, asset
attach. They all pass through the sequencer as `Semantic(command)` and run
like this:

1. **Evaluate at head.** The sequencer computes what the command needs from
   the cache: the anchored range and quoted text for a comment, the patch
   applicability for a proposal, the tree digest for a label. Preconditions
   that fail return a typed conflict and stop here. Nothing was written.
2. **Open one transaction** with the existing fencing: `deployment_writer FOR
   SHARE` and epoch check, document row `FOR UPDATE`, current authority.
3. **Flush inside it.** If the buffer is non-empty, insert its row and
   advance `update_sequence`. The command's `source_sequence` is the head
   sequence after this step.
4. **Write the command's rows** with `source_sequence` and, where the command
   names a state, the head frontier.
5. **Commit.** Then acknowledge the flushed batches, send the command's
   result, and broadcast its event.

The comment and the source it quotes are in one transaction, so no comment
ever names text PostgreSQL does not hold. The projection used to anchor it is
the same head the row captured, because the sequencer processes nothing else
between step 1 and step 5.

### 7.1 Preconditions

| Command | Precondition checked at head |
|---|---|
| Comment on source text | quoted `exact` with context resolves to exactly one range in the named file |
| Comment on rendered text | render token names a sequence at or after the last flush that changed source; otherwise stale-selection |
| Proposal decide | proposal base and tip, or patch context, still match |
| Restore | `expected_sequence` equals head sequence |
| Label | none; records head sequence and frontier |
| Agent patch | each expected passage matches at head |

### 7.2 Idempotency

Creates carry a client-generated UUID as primary key (`temp_id` already exists
for comments) and are inserted with `ON CONFLICT DO NOTHING RETURNING`; a
retry returns the existing row. Updates and decisions are conditional on a
version column and return the current row on mismatch. There is no receipts
table.

### 7.3 Commands that produce source

Restore, agent patch, proposal acceptance and whole-project replacement change
the text. The server is then a peer of its own log:

1. `let draft = cache.fork()`; apply the edit to `draft`.
2. `let update = draft.export(Updates { from: cache.oplog_vv() })`.
3. Run steps 2 to 5 above with this update appended to the buffer as a batch
   from the deployment's own peer key, so it lands in the same row as the
   flush and the same transaction as the relational effects.
4. Import `update` into the cache and relay it to editors like any batch.

The deployment peer id is stable and recorded in `server_runtime_state`.

## 8. Storage model

### 8.1 Kept

`accounts`, `documents`, `grants`, `share_links`, `document_marks`,
`annotations`, `replies`, `document_assets`, `document_proposals`,
`document_proposal_hunks`, `document_bases`, `document_updates`,
`deployment_writer`, `schema_metadata`, `server_runtime_state`,
`storage_usage` and its triggers.

### 8.2 Changed

- `document_updates`: framed rows as in 4.2.1; `state_bytes` dropped.
- `document_checkpoints` becomes the only label table: `id, document_id,
  sequence, source_sequence, frontier, tree_digest, label, reason, author,
  created_at, archive_key nullable`. `archive_key` is filled when an export
  is first requested, never at label time.
- `annotations.source_revision` becomes `source_sequence` and is always set
  for new rows; `checkpoint_id` keeps legacy evidence for old rows.
- `documents`: `update_sequence` is the head row; `commit_sequence`,
  `source_revision`, `current_version_id`, `current_bundle_id`,
  `project_generation` dropped.

### 8.3 Dropped

`document_source_revisions`, `document_command_receipts`,
`document_activity`, `document_versions` (folded into checkpoints),
`bundles`, `bundle_files`, `jobs`, `maintenance_cursors`,
`annotation_live_state`, `annotation_revision_migration_exceptions`.

### 8.4 Compaction

When a flush leaves `uncompacted_update_count` at or above 100 or
`uncompacted_update_bytes` at or above 16 MiB, the sequencer schedules
compaction on the in-process worker. Compaction uses the warm cache when
present: `cache.export(Snapshot)` at the flushed head, write the blob, then
`activate_collaboration_base(through = head)` in one transaction, which also
deletes the rows it covers. With no warm cache it builds one under the
projection bounds. Compaction never replays history when a cache exists, so
its CPU cost is one export.

### 8.5 Archives and export

A plain-source archive is produced on request from `Project(cut)`, written to
the object store keyed by tree digest, and its key cached on the label row.
Backups are PostgreSQL plus referenced blobs, as today; a restored deployment
rebuilds every cache from the log.

### 8.6 Background work

There is no jobs table. The in-process worker holds a bounded queue of
compaction, archive and deletion tasks. Work that must survive restart is
represented by durable state, not by a queued row: `documents.status =
'deleting'` with `deleted_at`, `uncompacted_*` counters above threshold, an
`archive_key` still null on a label whose export was requested. At startup the
worker scans those and requeues. Timers: compaction and deletion sweep every
60 s; nothing runs every second; an idle deployment issues no queries.

## 9. Limits and resources

Encoded bytes bound ingress and storage. They do not bound decoded memory,
rebuild time or fan-out, so those are bounded separately.

| Resource | Bound | On breach |
|---|---|---|
| One update | 4 MiB | refuse the frame |
| Buffer per document | 1 MiB or 30 s | flush |
| Log per document since base, plus base | configured quota (default 64 MiB) | refuse new updates with a retryable close and a quota reason; comments still work |
| Per-principal updates | token bucket, existing rate | delay then refuse |
| Cache total | configured bytes (default 512 MiB) | evict cold entries; refuse a build that would exceed it with `busy` |
| Cache build | one at a time per document; `min(cores, 4)` concurrent; 10 s wall clock; 4 times the document's log bytes in memory | mark the document unreadable |
| Projection request | served from cache in microseconds; a cut older than head forks and is bounded like a build | `busy`, retryable |
| Subscriber queue | existing byte and frame caps | close that subscriber |

A document marked unreadable answers projection requests with 503 and a
reason. Editors keep their sockets and may still type; their batches are
buffered and flushed like any other, because the log needs no cache. The mark
clears when a build succeeds, which an operator can trigger after raising a
limit or after an editor's export and re-import.

## 10. Failure modes

| Failure | Behaviour |
|---|---|
| Process dies with a buffer | Section 6.4. No data loss beyond what no client retained |
| PostgreSQL unavailable | Relay continues; flushes fail and retry with backoff; buffer grows to its cap, then editors get `sync delayed` and further updates are refused retryably; semantic commands are refused |
| Writer lease lost | Existing fence: refuse ingests and commands, close sockets with a reason; a new owner rebuilds caches from the log |
| Bytes are not a Loro update | Dropped at ingest; sender told; not appended |
| Valid update, bad structure | Appended; projection rule renders it deterministically; nothing repaired |
| Causal gap in an update | Accepted; hidden until dependencies arrive from the sender's resend |
| Cache build exceeds bounds | Document unreadable; typing continues; operator recovery |
| Subscriber too slow | Closed; reconnects with `after` |
| Duplicate batch after lost ack | Loro discards known operations; the row holds a few redundant bytes |
| Two labels at the same head | Two rows; deduplication is by client id only |

## 11. Client changes

- `acknowledge(upTo, sequence)`: clear the map up to `upTo`; persist
  `lastSequence`. Delete `confirmedCoverage` and vector handling.
- `open()`: send `after: lastSequence`. Delete `doc-sync` and `catchUp`'s
  vector branch; resend the unacknowledged map after import instead.
- Handle `doc-rows` as a single import.
- Readers: on `source-changed`, refetch the projection with the etag as
  today.
- Apply the projection rule from section 4.4 when building the render tree
  and the file panel, from the shared fixtures.
- Local persistence (IndexedDB) is unchanged. Whether its breadth stays is a
  separate decision (`REVIEW-BIG-IDEAS.md` 2.4).

## 12. What this deletes

| Today | Disposition |
|---|---|
| `RoomState`, `Session` and its 14 fields, `Measured<T>`, `resident.rs` | Deleted; the sequencer holds five fields |
| `DocumentOwner` lease and every `acquire()` site | Deleted; the actor's inbox is the order |
| `Room::receive_update` fork, repair, validate, measure, install | Deleted; ingest is import, buffer, relay |
| `document/session/repair.rs` | Deleted; section 4.4 replaces it as a read rule |
| `librepaper-document-core` final-candidate validation and history measurement | Deleted; the crate keeps the schema constants and the projection rule |
| `commit_source` for typing, `PreparedSource`, encoded history accounting | Replaced by the flush row; `commit_source`'s fencing skeleton is reused by section 7 |
| `document_command_receipts` and `SemanticReceipt` | Deleted; section 7.2 |
| `document_source_revisions` | Deleted; frontier lives on rows and labels |
| `storage/worker.rs` job claiming, `postgres/jobs.rs` | Deleted; section 8.6 |
| Room sweep, per-socket tick, link reauthorizer loop, cost checkpoint | Deleted; link expiry is a per-connection deadline; see `REVIEW-BIG-IDEAS.md` 2.7 for the rest |
| `doc-sync`, vector coverage in acks, `begin_open_state` | Replaced by section 6 |
| `checkpoint_archive` job, eager archives | Replaced by section 8.5 |
| `bundles`, `bundle_files`, `bundle_cleanup` | Deleted |

Kept: the lease and epoch, `Authority` checks, the two-origin model, share
links, quota triggers, blob store, compaction bases, and browser rendering.

## 13. Migration

Hard cutover, per project practice. One migration:

1. Rewrite each existing `document_updates.update_bytes` into a one-batch
   frame with peer key `migration:legacy` and client seq equal to its
   sequence. Frontier is already present for post-0002 rows; rows without
   one get the frontier of a rebuilt document at that row, computed once by
   the migration tool.
2. Build `document_checkpoints` rows from existing checkpoints with
   `source_sequence` from `document_source_revisions` where mapped;
   unmapped legacy rows keep their archive key and a null frontier.
3. Copy `annotations.source_revision` into `source_sequence` where set.
4. Drop the tables in 8.3 and the columns in 8.2.
5. Clients: no compatibility path. An old client's `doc-sync` receives
   upgrade-required and keeps its local state.

## 14. Verification

### 14.1 The spike, before anything is deleted

A branch implementing only sections 4.1 to 6 plus the comment command, run
against disposable PostgreSQL 17 and headless Chromium:

1. Three editors type continuously into one 1 MiB document for ten minutes
   with the 30 s flush. Record rows written, relay latency p50/p99, server
   CPU, server RSS, and cache rebuild time after a forced eviction.
2. Kill the server process with a non-empty buffer; restart; reconnect all
   three; assert every client and a fresh fourth client converge to the same
   text and every previously unacknowledged batch is now in a row.
3. Post a comment while another editor is mid-word; assert its
   `source_sequence` row contains the quoted text; kill PostgreSQL between
   step 1 and step 5 of section 7 and assert nothing was written.
4. A reader polls the projection every second throughout; record projection
   latency from cache and its etag churn.

The decision to delete the room engine is made on these numbers, not before.

### 14.2 Tests that ship with the cutover

- Projection fixtures (section 4.4) pass in Rust and JavaScript with
  byte-identical trees.
- Ingest refuses non-Loro bytes, accepts a causal gap, and hides pending
  operations until the dependency arrives.
- Flush triggers on each of the five conditions; buffer never exceeds its
  cap; acks name the row that holds the batch.
- Join with `after` receives exactly the missing rows plus the buffer; join
  after compaction falls back to full state; no row is lost between state
  and first relay under a concurrent flush.
- Duplicate resend after a dropped ack stores one extra row and no divergent
  text.
- Semantic transaction rolls back the flushed row with the command on
  failure; on success both are present.
- Restore and agent patch produce a server-peer batch in the same row as the
  flush and relay it.
- Cache eviction under pressure, unreadable marking on a bound breach, and
  clearing after a successful rebuild.
- Lease loss during a buffered typing session refuses further ingest and
  leaves the log consistent.
- Idle deployment for ten minutes issues no queries other than the lease
  keepalive.

## 15. Decisions this leaves open

- Proposals as patches instead of branches (`REVIEW-BIG-IDEAS.md` 2.2). This
  specification works with either; branches require `fork_at(base frontier)`
  from the cache, which section 4.3 provides.
- Bounded collaboration generations (2.3). Orthogonal; a generation boundary
  is a new base and a new sequence range.
- Offline breadth and multi-tab persistence (2.4). The client side of
  section 6 does not depend on it.
- Whether readers of heavy formats get a rendered artifact instead of a
  projection (2.11). The projection route stays either way.
