# 9. Stable checkpoint attribution

Status: proposed early correctness milestone, independent of profiling. Inherits
[umbrella section 9](../../../SPEC-refactor.md#9-persist-stable-checkpoint-author-identities-for-erasure).

## Scope and implementation

Persist an optional stable account ID separately from display attribution.
Inventory every checkpoint-writing path, including user requests, automatic
checkpoints, restore, import, and system operations. Define which authenticated
identity each path carries, and leave anonymous/imported/system attribution
explicitly unresolved where no authoritative account association exists.

Deliver schema migration, row conversion, writer changes, erasure queries, and
backup/restore compatibility together. Never infer an account ID from a display
handle. Backfill only from authoritative associations and document unresolved
legacy records as an erasure limitation.

Erasure clears the stable ID and associated identifying display metadata in
bounded restartable batches. Check erasing-account state at the durable write
boundary so a queued checkpoint cannot reintroduce attribution. Coordinate that
transaction with track 1 if the execution migration has already landed.

Inventory attribution in immutable objects and backups. Define replacement,
retirement, and restore behavior for each retained copy, including how restore
avoids reintroducing erased identity. Do not claim complete erasure for copies
outside the implemented protocol. Preserve document content and event identity.

## Acceptance and delivery

- Cover renames, reused handles, equal display names on distinct accounts, and
  anonymous, imported, system, and unresolved legacy records.
- Restart interrupted migration and erasure batches without skipping records.
- Interleave checkpoint creation and erasure at the final durable check.
- Backup/restore preserves identity distinctions and obeys the documented
  erased-attribution policy.
- One account's erasure never changes another account's attribution or retained
  document content. State remaining legacy/immutable-copy limitations explicitly.
