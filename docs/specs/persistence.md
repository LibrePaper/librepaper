# SPEC: batched edit persistence

## Decision

LibrePaper uses one batching implementation and one storage shape: a SQLite
catalogue and immutable objects, both in the deployment directory.

Live Yjs edits from many documents are combined into shared segments instead
of saving one snapshot per document. One coordinator, record format,
scheduling, replay, compaction and deletion code path covers all of it.

A save is acknowledged only after the segment is durable in the object store
and its recovery reference is committed in the catalogue: the deployment
disk.

## Records and batching

Each document has an immutable `storage_id` and increasing durable sequence.
Each update record contains its format version, document identity, sequence,
stable retry id and epoch, length and digest. Retries within an epoch are
idempotent; conflicting reuse of an id is rejected. Deletion-only updates and
server repairs are durable updates too. Admission reserves both payload and
bounded retry-index capacity before accepting a record.

The server issues retry epochs and records their identities in the recovery
graph. Segment indexes retain the id/digest/sequence mappings for open epochs;
compaction preserves these mappings until it closes the epoch. Closing requires
all admitted records of that epoch to be durably covered, and blocks admission
under it before the graph transition. Requests from a closed epoch receive an
explicit resynchronization result; they are never admitted as new requests.
Clients rejoin against the recovered CRDT and send only missing updates under
the new epoch. This bounds retry metadata without replaying an old operation
after its receipt was compacted away. Compound product-operation receipts follow
the catalogue's longer lifetime instead.

One in-process coordinator owns queues, scheduling, segment construction,
retries and confirmed sequence positions. It groups all dirty rooms into a
flush round, then splits the round into bounded immutable segments when needed.
Limits on queued bytes, records, segment size and per-room contribution are
chosen from the 1,000-active-document prototype. Apply backpressure before
accepting work that cannot fit.

Recovery bases encode the existing CRDT, including its identities and deletion
state. Replacing document content or accepting a suggestion produces a
server-authored update against that CRDT, using the compound transaction in
[catalog.md](catalog.md#publication-and-reclamation). Compaction never creates a
fresh CRDT to discard synchronization metadata. Initial document creation is the
only case that starts with a new identity and base.

The initial regular flush floor and maximum dirty-age target are 15 seconds.
Two seconds of quiet may request an earlier flush only when the floor permits
it. The deadline does not reset on each edit. Storage outages can extend the
delay; unsaved work remains queued and is never reported as saved.

## Publication protocol

Segments live under a private, versioned `journal/<deployment-id>/` namespace
in the local object directory.
They contain bounded indexes so recovery can find one document's frames without
scanning unrelated payload. They are never exposed through document routes or
public URLs because one segment may contain several users' private text.

Publish a flush as follows:

1. Seal the batch and hold lifecycle gates for every included document, followed
   by the journal publication gate, in the order specified in `catalog.md`.
   Reserve its storage and commit a journal preparation containing its expected
   head, object plan, covered sequences and allocations before object writes.
2. Durably write immutable segments. An object write alone is not success.
3. In one authoritative catalogue transaction, check the expected journal head,
   insert segment descriptors and advance the head. Commit any attached product
   metadata and operation receipts in this same transaction and remove the
   preparation.
4. Acknowledge each included sequence only after the transaction is known to
   have committed.

No object-store request occurs inside a SQL transaction. Write local objects by
temporary file, file `fsync`, atomic rename and parent-directory `fsync` before
publishing their references. Synchronize newly created directory entries too;
propagate every write/sync error. Flush directory removals before confirming
physical reclamation and releasing local capacity. Atomic rename alone does not
establish crash durability. See the [Linux fsync contract](https://man7.org/linux/man-pages/man2/fsync.2.html).
After an unknown SQL outcome, reconcile stable segment ids and the head before
retrying or acknowledging.

Do not publish another journal transition while the outcome of the preceding
one is unknown. Read preparation, head and descriptors in a consistent primary
transaction. A matching committed descriptor/head proves success; a surviving
preparation with the unchanged expected head permits retry with the same object
ids. Any other combination requires reconciliation before acknowledgement or
cleanup. Process restart follows this rule before serving recovered rooms.

## Durable record contract

The following records are part of the shared catalogue migration. SQL spelling
may follow driver conventions, but these fields, uniqueness constraints and
atomic transitions are required. Identities, digests and keys are text; revisions,
sequences, byte counts and Unix timestamps are nonnegative integers. Structured
metadata uses versioned JSON with a 256 KiB encoded limit per record; split a
preparation before admission if its plan exceeds that bound. No record contains
document bodies. Every relation uses bound and paginated queries.

| Record | Required fields and indexes |
| --- | --- |
| `journal_state` | Singleton primary key, immutable `deployment_id`, `writer_generation`, increasing `revision`, `last_operation_id`, `next_segment_seq`, committed `manifest_key`/digest/length and `tail_after`. |
| `journal_preparations` | Primary key `operation_id`, kind (flush, compaction, creation, deletion), expected revision/generation, creation time and plan. The plan lists output keys/digests/lengths, covered document sequences/epochs, product-operation ids, protected input keys and reservation allocations. At most one unresolved journal preparation. |
| `journal_segments` | Primary key `segment_id`, unique increasing `segment_seq`, originating operation id, unique object key, digest, encoded byte count and commit time. Index `segment_seq` for bounded tail reads. |
| `journal_retirements` | Primary key object key, kind, encoded bytes, per-identity payload/maintenance allocations, retired revision, modification time, first-unreferenced time and `delete_after`. Index `(delete_after, object_key)` for cleanup. |

Retirement metadata deliberately has no cascading foreign key to documents:
shared objects and unknown-identity orphans outlive individual catalogue rows.
For unknown orphans, retired revision may be absent; the catalogue's orphan
grace and primary-reference checks still apply. Keep retirement metadata until
physical deletion and allocation release are confirmed in one SQL transaction.

The committed manifest is an immutable root with bounded shard descriptors
(partition, key, digest and length). Shards map `storage_id` to a recovery-base
key/digest/length, covered durable sequence, retry epoch/index references and
per-object payload allocations. Base descriptors preserve the source state
needed to compare journal coverage with catalogue checkpoints. A complete shard
set, including an empty set for initialization, is published before the root
becomes the authoritative `manifest_key`.

Segment objects carry bounded indexes mapping `(storage_id, epoch)` to sequence
ranges, frame offsets/lengths, retry ids/digests and attributable payload bytes.
For a document, replay only records above its base's covered sequence. The
manifest's `tail_after` and the catalogue's matching value advance together;
all retained records through that segment sequence must be represented by the
new graph before advancing it. Gaps, conflicting sequences or bad digests fail
recovery for the affected document instead of silently dropping saved work.
Serving-host lookup maps are disposable caches of these committed records.

Updating the root, tail boundary, replacement descriptors, preparation state,
product metadata and retirement jobs is one guarded SQL transaction. Remove
obsolete tail descriptors only as that transition establishes equivalent
coverage; their objects remain queued for safe reclamation. Neither recovery
nor a cold document open scans historical descriptors below `tail_after`.
Limits on manifest shard size, retained tail descriptors/bytes, per-document
replay and retry-index entries are explicit configuration validated by the
prototype. Stop new admission before exceeding a limit if compaction cannot
advance.

## Recovery and compaction

A recovery base stores complete Yjs state and the sequence it covers. It is
separate from user-visible history and preserves CRDT identities. Create bases
before global or per-document replay limits are exceeded, including for rooms
that never become quiet.

Compaction writes replacement bases, segments and manifest shards first, then
publishes the new graph and retirement metadata in one guarded head transition.
Keep the old graph until the new one commits. A crash must recover either graph
without losing acknowledged work.

Recovery rebuilds from the committed manifest and bounded tail without
scanning the lifetime journal. Process recovery first uses its deployment
disk; disk-loss recovery uses a completed matched backup.
Restoring an older catalogue uses a named complete backup under
[catalog.md](catalog.md#secrets-and-recovery), including its independent object
copies and matching secrets. It does not depend on the failed host's disk.

## Deletion and accounting

Deleting a document first blocks access and new edits, then resolves any batch
that contains its admitted frames. Rewrite shared objects to retain other
documents before retiring the originals. The immutable `storage_id` prevents
old frames from reviving a reused slug.

Retired objects are deleted only after committed references, active readers and
prepared operations no longer need them and the configured document recovery
window has elapsed. Recheck under the applicable lifecycle/publication gates
before deletion. Never reuse a journal key once queued for retirement; write a
fresh immutable key if its content is needed again. Shared journal cleanup is
owned by the journal, not by document-prefix cleanup. A deleting document keeps
its slug until its attributable journal payload is reclaimed. Account erasure
reports live-store reclamation separately from expiry of independently retained
backup copies under the disclosed policy.

Charge each document for its retained base and frame payload, including
temporary and retired copies until physical reclamation. Use the catalogue's
separate maintenance reserve for bounded rewrites when ordinary quotas are full;
persist allocation changes with the preparation and release them only after
reclamation. Other owners need no spare quota to permit one document's deletion.
Track shared framing, indexes, catalogue storage and backups separately. Never
release reserved bytes before reclamation succeeds.

## Cost and release checks

The request model is:

`flush rounds × segments per round + bases + compaction + history + assets + renderings + backups + retries`

This counts object writes. Measure filesystem writes and catalogue operations
separately, including cleanup and restore traffic. Empty flush rounds write no
segment. This is a baseline, not a complete measured cost.

Before release, measure a normal workload and 1,000 continuously active
documents. Publish the document-size distribution, edit trace, annotations,
sockets, configured room count/byte caps and observed peak heap/RSS. Start the
1,000-document case with roughly 100 KiB text per document, set `rooms_max` to
at least 1,000 and measure the required byte cap. This is a capacity target
for a declared configuration, not a promise that 1,000 maximum size documents
fit the default 512 MiB. Separately stress the defaults with large documents
and require bounded memory and explicit admission refusal.

Record segment counts and bytes, queue pressure, save/history latency, replay
cost, object operations, catalogue writes/reads, recovery time, refusal rates
and maintenance progress at full ordinary quota. Include receipt/index
overhead, repeated cold starts, backups and all checkpoint reasons.

Fault tests cover lost and ambiguous SQL responses, duplicate and
deletion-only updates, corrupt or missing segments, crashes during compaction
and deletion, retries across epoch closure, compound source/metadata
publication, slug reuse, cross-document privacy, bounded queues and replay,
disk-full and directory-entry failures, disposable-cache loss, and local
backup restore after live-object deletion. File-backed tests are required by
`catalog.md`.

[catalog.md](catalog.md) defines catalogue transactions, quotas and lifecycle
state.
