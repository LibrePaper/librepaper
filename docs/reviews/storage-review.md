# Storage review reconciliation

This combines the six reproduced findings from the first storage review with the findings supplied from the second review of `df61a2a`. Fixes are evaluated against `live-markdown-editor`, starting at `340a376`; concurrent room/catalogue work is kept separate. The shared Claude artifact loaded its shell, but its content endpoint returned HTTP 403. Findings not present in the supplied summary have not been assessed.

The severity totals from separate reviews are not added: several findings describe the same root cause. Reproduction and production reachability determine which changes belong in this patch.

| Finding | Assessment and disposition |
|---|---|
| Recovery-base JSON expansion prevents compaction and repeats saves | Critical; fixed with a bounded binary encoding and legacy JSON decoding. Regression covers a snapshot above 4 MiB. |
| Prepared journal failure blocks unrelated rooms until restart | High; failures reconcile under the publication gate and retry in-process. Known aborts discard the rejected identity while preserving other queued records; uncertain storage failures remain fail-closed. |
| Account-erasure cursor crosses table boundaries | High; reset on every stage transition. Tests cover lower-sorted guest rows and the different reply cursor shape. |
| Sealer misses 74 framing bytes and drops drained records | High; exact codec sizing, per-document splitting, and queue restoration. A timeout regression also checks that a failed seal releases runtime locks. |
| Valid compacted backups fail manifest digest verification | High; fixed canonical descriptor verification rather than raw-object SHA-256. Compacted backup round-trip passes. |
| Deleted compacted document remains referenced by manifest | High; retirement atomically invalidates the derived manifest snapshot and queues its objects. SQL retains surviving bases/segments; the next compaction rebuilds manifests. Deletion and surviving-document recovery are tested together. |
| Active-document rendering retirement is never processed | High; worker handles retired renderings, guards against publication/reservation races, and removes physical-byte accounting after deletion. |
| Deletion strands a prepared product publication | High; withdrawal aborts prepared receipts in the same transaction and keeps physical bytes charged until reclamation. |
| Measurement consumes maintenance headroom | High; measured bytes, ordinary reservations, and maintenance borrowing are additive. Reconciliation shares the ledger-aware measurement path. |
| Filesystem temporary files are exposed to garbage collection | High; atomic temporary names are reserved and excluded from both listing paths. |
| Journal readers race physical reclamation | High; a catalogue-owned async gate is automatically shared by runtimes, recovery, deletion, and retirement workers. Existing explicit reader leases remain honored. |
| One bad retirement blocks independent reclamation | High; failed rows are logged and delayed, allowing later jobs through even with a one-row batch. The affected document retains its charge until its own reclamation succeeds. |
| Legacy format-1 records become undecodable when re-encoded | Medium; format-1 segments retain their original header/layout when rewritten. Round-trip regression passes. |
| Blob backup treats manifest read errors as absence | High API defect; only NotFound permits cleanup or creation. Objects and completion markers use create-only publication. Transient-read regressions preserve existing bytes. This API has no production CLI caller. |
| Routine compaction retirements prevent offline backup | Medium; committed retirement rows are allowed; unresolved operations and lifecycle transitions still block backup. |
| Restore briefly creates secrets with default permissions | Medium; private directories and files are created before secret contents are written. |
| Visitor rendering publication ignores owner key | Medium; supplied visitor ownership must match the catalogue, and the document must be active. The upload caller preserves the synthetic catalogue identity of unowned documents after route authorization. Internal publication also checks lifecycle. |
| Catalogue methods synchronously block Tokio workers | Architectural follow-up. The synchronous API and its callers need an executor-boundary change, not mechanical spawn_blocking around nested transactions. No claim that every read performs fsync. |
| S3 listing pages scan the entire prefix | Medium; native pagination follows continuation tokens until the requested page is filled or the listing ends. Mock-server regression covers S3's 1,000-object request limit. Hosted serving is explicitly disabled in this build. |
| S3 retries and batch deletion are absent | Operational/performance follow-up, not by itself a correctness defect. Ambiguous-write retry semantics and partial batch errors need an explicit contract. |
| Checkpoint attribution uses display handles, erasure matches ids | Valid data-model concern; historic mutable handles cannot be safely converted to immutable account identities by guessing. Requires a separate persisted author-id field and caller migration while retaining display attribution. |

## Validation

The original baseline passed 39 storage unit tests and 12 blob contract tests. Six additional probes reproduced the first review's defects. The integrated focused suite passes 64 storage tests. Final validation, after rebasing onto the separately committed room fixes at `328a45b`, passes all 808 workspace tests (one ignored), `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --offline -- -D warnings`. Generated web assets were rebuilt for the workspace tests. Uncommitted document/frontend work remains separate.

Cross-review also added durable retirement records for staged erasure-rewrite outputs before object writes, conservative accounting after ambiguous writes, and recovery of maintenance borrowing associated with interrupted compaction preparations. A deterministic deletion regression caught and fixed a shared segment retaining the deleted document's representative storage identity after its bytes were rewritten.

## Remaining work and limits

- Move synchronous catalogue access across an appropriate blocking-executor boundary. This requires a caller-level concurrency design; transactions should remain intact.
- Persist stable checkpoint author IDs and migrate callers. Display handles are not reliable historical account identifiers, so erasure cannot safely guess their identity.
- The existing `--max-size` setting permits 100 MiB while the journal's default queued payload budget is 64 MiB. Recovery bases now support the journal's 64 MiB aggregate bound; larger configured documents need coordinated admission/queue limits.
- The unused blob backup API requires external serialization of creation and cleanup for the same backup ID. Fresh NotFound checks fail closed on read errors but cannot make an unconditional deletion atomic with concurrent manifest publication. The local CLI backup path has its separate offline lock.
- S3 retries and batch deletion remain operational follow-ups. Retries must distinguish ambiguous writes, and batch deletion must preserve per-object failures.
- The shared journal gate implements the current single-process local authority contract; it is not a cross-process or hosted reader-lease protocol.

The inaccessible portions of the external report, including its other medium/low/nit findings, remain unassessed. Its severity totals are not presented as independently verified totals.
