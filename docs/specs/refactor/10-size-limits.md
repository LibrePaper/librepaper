# 10. Admission and persistence size limits

Status: proposed first correctness milestone. Inherits
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
