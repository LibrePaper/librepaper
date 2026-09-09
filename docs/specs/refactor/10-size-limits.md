# 10. Admission and persistence size limits

Status: implemented and merged. Inherits
[umbrella section 10](../../../SPEC-refactor.md#10-align-document-admission-with-journal-and-recovery-limits).

## Decision and scope

Choose rejection of unsupported work for this delivery. Keep the current
segment format, existing record chunking, and binary/legacy recovery decoding.
Do not increase global constants to claim support for 100 MiB source documents.
Larger recovery bases and a streaming persistence pipeline require a separate design.

The current default source ceiling is 4 MiB; configuration accepts up to
100 MiB, while the default journal payload queue and recovery payload ceiling
are 64 MiB. Existing chunking splits records but still retains a complete
snapshot and copied fragments; it is not an aggregate memory bound.

## Limit contract

Define a shared persistence-limit policy used by configuration, publication,
room mutation admission, journal append, and compaction:

- `S`: configured source-byte ceiling. Preserve the 4 MiB default.
- `E`: supported encoded snapshot ceiling, no greater than either the recovery
  payload ceiling or the configured aggregate journal payload capacity.
- `Q`: aggregate queued plus executing journal payload budget. Count capacity
  until the operation releases ownership, not merely until dequeue.
- `M`: memory admission for snapshot construction, staging, encoded bodies,
  record copies, and compaction overlap. Distinguish it from storage quota.

Reject configurations with `S > E`, invalid framing/metadata limits, or budgets
unable to process one maximum supported snapshot. `S <= E` is a conservative
configuration policy for this delivery, not a theorem relating source bytes to
encoded bytes. It does not guarantee that every document below `S` fits:
CRDT history and metadata
can grow independently of visible source. Enforce both source and encoded
ceilings for each proposed mutation and explain both in CLI/server documentation.

Before implementation lands, record the chosen default `E`, `Q`, and `M`, with
an allocation/copy inventory and boundary measurements. The existing 64 MiB
payload constants are upper constraints, not evidence that a 64 MiB snapshot
fits a particular total memory budget. Validate fragment count, framing, and
metadata separately; use checked arithmetic for all derived bounds.

## Admission and failure behavior

1. Acquire bounded memory admission before making large snapshot copies.
   Bound waiting producers too; a task waiting with a full snapshot is not free.
2. Evaluate the candidate state in bounded staging or using a proven conservative
   bound. Reject before applying or broadcasting the mutation to the shared
   document. Account for generic Yjs updates, restore, suggestions, and imports.
3. Distinguish a permanent size refusal from temporary queue/budget saturation.
   Capacity saturation must not be reported as a document that can never fit.
4. Preserve authority checks and quota reservations through persistence. Transfer
   budget ownership to executing work and release on confirmed settlement,
   including cancellation. Never acknowledge an update that cannot be saved.
5. Keep existing persisted documents readable and recoverable within existing
   format bounds even if a new write policy is stricter. Refuse incompatible new
   writes explicitly; do not truncate, delete, or enter an automatic retry loop.

Audit configuration, server/CLI publication, room edits, checkpoint/restore,
suggestion acceptance, journal append, compaction, backup, and recovery callers.
Deliver narrow typed permanent/temporary errors here without waiting for track 6.

## Acceptance and delivery evidence

- Exercise source and encoded sizes independently at 4 MiB, 64 MiB, the new
  supported maximum, and just over each applicable bound. Test the formerly
  accepted 100 MiB configuration produces the intended configuration error.
- Include small visible source with large CRDT history and metadata, and a
  candidate update that would exceed the encoded ceiling after application.
- Every accepted boundary snapshot appends, receives its durable acknowledgement,
  compacts, backs up, restores, and recovers after restart with identical content.
- Concurrent large rooms and compactions respect `Q` and `M`, including snapshot
  staging and fragment copies. Use allocation/budget counters and deterministic
  barriers, not only process-memory samples or timing assertions.
- Cancellation, queue saturation, and storage failure do not leak reservations,
  lose quota, broadcast refused updates, or repeatedly retry permanent failures.

Record numeric defaults and measured peak components here before merging.

## Implementation evidence

### Chosen numeric defaults

| Symbol | Value | Where it lives | Why |
| --- | --- | --- | --- |
| `S` default | 4 MiB | `Configuration::max_document` | unchanged |
| `S` maximum | 8 MiB | `config::SUPPORTED_MAX_SOURCE_BYTES` | `--max-size` above this is a startup error; 100 MiB used to be accepted |
| `E` | 16 MiB | `config::DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES` | inside the 64 MiB recovery-base ceiling and the 64 MiB queue budget, with room for history and metadata above a maximum source |
| `Q` | 64 MiB | `config::DEFAULT_MAX_QUEUED_PAYLOAD_BYTES` | the coordinator's existing aggregate queue, now counted until an operation settles |
| `M` | 512 MiB | `config::DEFAULT_MAX_STAGING_BYTES` | eight simultaneous copies of one maximum snapshot, times four concurrent operations |
| recovery decoding | 64 MiB | `MAX_RECOVERY_BASE_PAYLOAD_BYTES` | unchanged, as the decision required |

`S <= E` is a configuration policy, not a theorem. An 8 MiB source is not
proven to encode below 16 MiB; it is the largest source ceiling for which the
inventory below leaves room for the history and metadata that grow beside the
visible text. The delivery therefore enforces `E` directly rather than
inferring it from `S`.

### Allocation and copy inventory behind `M`

`config::SNAPSHOT_COPY_FACTOR` is 8. One append of an `E`-byte snapshot holds,
at its peak:

1. the snapshot `write_session_inner` encodes;
2. the clone it hands to `JournalRuntime::append`;
3. the record fragments `JournalRecord::chunked` copies out of it;
4. the flattened `record_payload` the idempotence check compares against;
5. the framed segment bodies `write_segments` encodes;
6. the clone each framed body is put to the object store with;
7. the recovery-base body a compaction builds from the same snapshot;
8. the framed base bytes and manifest shards that overlap it.

A compaction that follows an append in the same flush therefore overlaps
items 1-6 with 7-8, which is why the factor covers both rather than the append
alone. With `E` at 16 MiB one operation is admitted for 128 MiB, and `M` at
512 MiB admits four concurrently; a fifth waits, and past a waiting set of `M`
the next producer is refused temporarily rather than parked holding a copy.

Framing is validated separately and arithmetically:
`SEGMENT_HEADER_BYTES + MAX_RECORD_CHUNK_BYTES + record_framing_bytes(MAX_JOURNAL_IDENTITY_BYTES)`
must fit `MAX_SEGMENT_BYTES`, the fragment count for one maximum snapshot must
fit `u32` and `MAX_RECORDS_PER_SEGMENT`, and `MAX_METADATA_BYTES` must be
positive and inside a segment. Every derived bound uses checked arithmetic:
`PersistenceLimits::validate` returns a configuration error rather than a
wrapped comparison for ceilings near `usize::MAX`.

### Before and after

- Before: `--max-size` accepted 1..=100 MB. After: 1..=8 MB, and
  `PersistenceLimits::validate` runs in `ServiceFlags::configuration` and again
  in `JournalRuntime::new_with_policy`, so an unsupported configuration fails
  at startup rather than at the first oversized save.
- Before: room admission bounded source bytes only, so a document inside
  `--max-size` could reach a snapshot the queue could not hold or a recovery
  base could not decode, and it was applied, relayed and later acknowledged.
  After: `receive_update` carries an upper bound on the encoded snapshot beside
  the document, decides the ordinary keystroke by comparison, and buys the
  exact answer on a scratch copy only when that bound cannot decide. A
  candidate past `E` is refused before it is applied or relayed. Suggestion
  acceptance decides on the scratch state it already builds; `set_main_file`
  rehearses a publish that would cross the ceiling; `write_session_inner`
  refuses an oversized snapshot before any journal work and fences the room so
  the sweeper cannot retry a permanent failure in a loop.
- Before: queue saturation and a permanently oversized record shared
  `JournalError::Limit`. After: `JournalError::Busy` carries temporary
  capacity, with `is_permanent`/`is_temporary`, and `config::WriteRefusal`
  gives permanent size and temporary capacity separate wordings. A capacity
  refusal always says to try again; a size refusal never does.
- Before: `seal` released a round's payload bytes before any object was
  written, so `Q` bounded what was waiting rather than what the process held.
  After: `begin_executing` charges a sealed round to `Q` until an owned guard
  is dropped, which happens on every exit from `append`, cancellation included.
- Before: nothing bounded persistence memory. After: `MemoryBudget` admits by
  bytes before the first large copy in `append` and `compact`, and before the
  scratch rehearsal in room admission (with `try_acquire`, because that caller
  holds the room state mutex and must not park there).

Existing persisted documents remain readable and recoverable: nothing in the
segment, record, manifest or recovery-base formats changed, and the ceilings
are checked on new writes only. A stored document already past `E` opens and
reads; its next write is refused explicitly, and nothing is truncated,
deleted, or retried in a loop.

### What the tests cover

`crates/librepaper/src/tests/size_limits.rs` (22 tests, 1 ignored):

- Policy: the shipped defaults validate; the supported maximum is
  configurable; `--max-size 100` is a configuration error and does not clamp;
  one megabyte past the maximum is refused; `S > E`, an `E` past recovery
  decoding, a `Q` or an `M` too small for one snapshot, and ceilings near
  `usize::MAX` are all refused; 64 MiB validates and 64 MiB + 1 does not.
- Admission: the source ceiling holds at its boundary and one byte past it; a
  document whose visible source stays under a kilobyte is still refused by the
  encoded ceiling once its history grows, with the shared document unchanged
  and nothing relayed; the journal refuses a snapshot past `E` permanently.
- Budgets: memory is admitted by bytes and released on drop; a save larger
  than the whole budget is permanent rather than parked; the waiting set is
  bounded and a cancelled waiter gives its place back and takes no budget; a
  sealed round stays charged to `Q` until its guard drops; two concurrent
  appends stay inside `M` measured by the budget's own peak counter; a refused
  save leaks neither queue nor memory reservation and does not wedge the
  runtime.
- Round trip: a boundary snapshot appends, is acknowledged with the peer's own
  sequence, compacts, and is read back byte-identically by a second process
  over the same storage with the session object deleted. The shipped 4 MiB
  default runs in the ordinary suite (about five seconds in a debug build); the
  8 MiB supported maximum is the same assertions and is `#[ignore]`d beside it.

### Remaining limitations

- Deployments without a journal (the legacy blob path) get `E` enforcement but
  no memory admission, because the budget is owned by the journal runtime.
- `set_main_file` and `add_text` have no error channel, so a publish refused by
  `E` returns an empty relay and logs; the callers already treated an empty
  return as "nothing to relay". A typed result belongs with track 6 or 7.
- A room fenced by the `write_session_inner` backstop stays read-only until it
  is reopened. That is the correct answer for a document that genuinely cannot
  be saved, but it is a blunt one for a transient accounting fault.
- The copy inventory is a static count read off the code paths, not an
  allocator measurement. The budget's peak counter is what the tests assert on.
