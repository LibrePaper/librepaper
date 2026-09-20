# Document architecture implementation ledger

Updated: 2026-09-19. Starting revision: `dfffc11330fafbe5223f5bf521ddc6a81efed476` (`main`). The untracked `SPEC-document-architecture.md` and `TODO-storage.md` were present at the start and were preserved.

This is executable-development evidence, not a production deployment report. No production database, provider bill, production network path, backup, cutover, or destructive migration was exercised.

## Inventory

The initial mutation inventory found:

- browser source mutations in `web/src/lib/project-session.js`, lifecycle/reconnect ownership split across `project-session.js`, `reader/collaboration.js`, `room.js`, and `Reader.svelte`;
- normal source mutation at `Room::receive_update`, followed by relay in `server/socket.rs`, with later `write_session` persistence;
- server/agent/import mutations guarded independently by `bundle_write`, `bundle_checkpoint`, `restore_write`, `session_write`, and direct `Room.state` access;
- proposal decisions written separately until the last decision, while `resolve_with_update` atomically coupled only final source append and proposal resolution;
- persisted mutable annotation attachment rows and `pending_attachments`/`flush_attachments` write-behind;
- an infallible `RoomSet::get` compatibility loader beside `try_get`;
- no deployment writer advisory lock/epoch implementation (only a migration lock and comments referring to writer ownership).

Baseline command: `cargo test --workspace --no-fail-fast`. Result before edits: 360 passed, 33 ignored, 0 failed in the library suite; all binary integration tests passed.

## Work-package status

### WP1 — atomic proposal acceptance: implemented

Implemented additive schema for deployment epoch, commit/source sequences, durable revision-to-frontier index, and lifetime semantic receipts. Added a concrete source commit transaction which locks in documented order (`deployment_writer FOR SHARE`, document `FOR UPDATE`, authorization rows), rechecks current editor authority, compares the expected update sequence, appends the exact Loro delta, records its frontier/revision, schedules compaction, and optionally stores a digest-bound receipt in one transaction.

Normal WebSocket source updates now prepare and validate one fork, commit it, then install/ack/broadcast the exact committed delta. Candidate rejection cannot mutate installed state. A committed update acknowledgement carries decimal commit/source sequence strings and Loro vector coverage. The old relay-before-persist behavior is no longer used by ordinary browser typing.

Executable disposable-PostgreSQL test: `writer_epoch_receipts_and_source_commit_are_fenced_and_atomic`. It proves I1 and I3–I6 for the new source transaction, including ownership contention, takeover fencing, matching receipt replay, and changed-payload refusal. `postgres_v3_contract` also passed after migration.

Proposal hunk decisions now use the same fenced transaction boundary. A partial decision writes its hunk, advances only `commit_sequence`, and stores its receipt. The final decision additionally compares the durable source sequence and reviewed tip, appends the exact accepted subset, records its immutable source revision, resolves the proposal and related annotations, schedules compaction, and stores the receipt before commit. The room installs and broadcasts only a newly committed final result; a replayed receipt is returned without reinstalling or rebroadcasting a candidate. The old independently committed `decide_hunk`, `resolve_proposal`, and `resolve_with_update` repository APIs were deleted.

Executable disposable-PostgreSQL tests: `proposal_decisions_are_atomic_idempotent_and_reauthorized` covers partial/final atomicity, lost-response replay, changed-payload rejection, and permission revocation without leaked hunk/source effects. The four proposal room round trips cover stale reviewed tips, complete and partial acceptance, and concurrent source edits.

Checkpoint creation, assets, and administrative mutations now enter fenced semantic boundaries. Multi-annotation agent commands enter one epoch-fenced document transaction: proposals, annotation upserts/deletes, and replies either commit together with one parent receipt and one `commit_sequence` advance or roll back together. Individual annotation commands retain lifetime receipts across every create/update/delete variant.

Comment and reply row mutations now acquire the writer-epoch lock, document lock, and current authorization in deployment/document/authority order, then advance `commit_sequence` in the same transaction. New annotation rows record the document's locked `source_revision`, so comment-only commits leave the source revision unchanged while binding original evidence to committed source. The PostgreSQL contract verifies this behavior. Persisted mutable attachment writes were deleted; attachment is recomputed as a disposable in-memory projection, while legacy rows remain readable during migration.

Suggestion creation and refinement now commit their proposal and annotation together. Standalone proposal open/update commands use the same fenced document transaction and lifetime digest-bound receipt table. Retrying an open returns the original proposal identity without another row or sequence advance; reusing the request identity with changed content is refused. Comment creation, replies, suggestion creation/refinement, resolve/reopen, and delete now also store lifetime digest-bound receipts atomically; retry returns the original result without another sequence advance, and changed content under the request identity is refused. Multi-item agent annotation batches use one parent receipt rather than independently durable child receipts. Duplicate row identities without a receipt remain content-checked no-op retries for compatibility.

HTTP whole-project replacements, agent patch edits, and checkpoint restores no longer apply a candidate to live room memory before storage. They use `Room::commit_edit`: fork, edit, repair, exact final validation, fenced `commit_source`, install, then broadcast. The HTTP archive/checkpoint is post-commit best-effort and cannot roll durable source back; both compensating rollback writers were deleted. Checkpoint creation no longer flushes collaboration state before archiving: it archives an already committed source revision. Legacy first-save and starter-bibliography repairs now prepare their delta during cold load and commit it through the epoch-fenced source transaction before the room is reachable; repair failure fences the room.

Direct agent suggestion acceptance now uses that same source transaction to append the exact source delta, resolve the annotation, and store the stable semantic receipt. The annotation is locked and checked before source counters change. Identical lost-response retry returns the stored agent result without reinstalling or rebroadcasting; changed content under the same request identity is refused. The disposable-database source contract proves success/replay and proves that a missing/stale suggestion leaves source counters, resolution state, and receipt storage unchanged.

Restore commits mark every pending proposal superseded and resolve its linked annotations inside the same source transaction. The prior standalone supersede repository writer was deleted. The restore integration test now proves the proposal transition alongside the restored source.

### WP2 — native core and browser protocol compatibility: revised by decision

Added `librepaper-document-core`, an I/O-free native-only crate owning schema roots, distinct revision types, exact final-candidate measurements, root/container/path validation, and final limits. The server source commit path calls it.

The 2026-09-19 architecture decision rejects Rust/WASM unification and any server-side browser compilation. The browser intentionally retains its existing single `loro-crdt` document and CodeMirror integration. Compatibility is tested through serialized protocol fixtures; no custom binding or mirrored document will be introduced.

### WP3 — durable commit benchmark: harness implemented, gate not accepted

Replaced the placeholder/bundle-era benchmark with `document_commit_release_benchmark`. It uses real Loro candidates, exact native-core validation, the production fenced `commit_source` transaction, 100 KB/1 MB/4 MB cases, 100 concurrent 100 KB documents, PostgreSQL WAL delta, and JSON output. Its default is ten minutes per size; shortening is explicitly recorded for harness smoke checks.

A one-second-per-size debug-profile harness smoke run passed on disposable local PostgreSQL 17. It is not release or representative evidence: it produced too few large samples and its timings fail the provisional gates. No ten-minute release run, managed-network run, provider cost input, or production budget was available. The 500 ms/1 s/2 s batching decision therefore remains unaccepted; production cutover is not authorized by this ledger.

Provisional local gates for the required benchmark: p95 commit under 100 ms, p99 under 250 ms, zero acknowledged-work loss, bounded resident candidate below twice the configured encoded-history ceiling, and no capacity failure at 100 active 100 KB documents. These are local engineering targets, not a production budget.

## Additional implemented boundaries

- Production startup now claims a session-scoped PostgreSQL advisory lock and transactionally advances the persistent epoch before constructing the reachable server. Each WebSocket source command probes the dedicated ownership connection, and every source transaction checks the epoch while holding a conflicting row lock. Dropping the lease detaches/closes the PostgreSQL session so a locked connection can never return to the pool.
- Source revisions remain independently resolvable through `document_source_revisions` after update compaction.
- Migration 0003 backfills one immutable source-revision row for every retained legacy update frontier before compaction, advances each document's counters above those revisions, and maps legacy `frontier:` anchors or checkpoint UUIDs only when their exact retained update sequence is provable. Every unmatched annotation retains its original evidence and gets a categorized `annotation_revision_migration_exceptions` row. `annotation_revision_audit` returns mapped and per-reason exception counts for cutover reporting. A staged PostgreSQL migration test (0001+0002 data, then 0003) proved one exact mapping, one explicit `frontier_not_retained` exception, and counter advancement.
- Browser API metadata now includes immutable `document_id`. New caches are keyed by origin and UUID. A legacy creation-time cache is moved only after an online response supplies the UUID; recreated slugs compare unequal. Prepared legacy caches remain recovery records.
- Remote confirmation is separate from local IndexedDB persistence. New acknowledgements must contain committed vector coverage before pending source work is cleared, and coverage is persisted in the local record.
- IndexedDB source persistence now uses immutable per-tab update records in a project-indexed store. Compaction reads the base and all included updates, writes the merged base, and deletes exactly those update records in one transaction; appends serialized after it remain reachable. Confirmation metadata rereads the current base instead of overwriting it from a stale tab-local record. A real Chromium two-session test writes concurrent operations, closes both tabs, and proves a third session recovers both.
- Asset registration now acquires the deployment epoch lock and reauthorizes the current editor inside the same document transaction that checks quotas and attaches immutable blob metadata. The room passes its mutation actor instead of discarding it. `asset_registration_is_reauthorized_and_epoch_fenced` proves revocation refusal, authorized success, and stale-process refusal after writer takeover.
- Administrative document writers now use the shared epoch-locked transaction: sharing links and grants, identity/ownership updates, version creation/backdating/labeling, trash/restore/purge, account erasure, and every durable job state transition. Consequently a worker from a superseded process cannot claim, retry, complete, or delete work after takeover. The writer fencing contract now explicitly checks stale sharing and job enqueue attempts in addition to source commits.
- Checkpoint identity is now an atomic relational event over one committed source revision rather than a synchronous archive row. Its transaction reauthorizes the editor, verifies the current source revision, records parent/tree/change metadata, advances `commit_sequence`, stores an optional lifetime receipt, and queues exactly one archive job. The room timeline and label/read APIs use `document_checkpoints`; pending reads return an explicit 202 state. A fenced worker reconstructs the exact retained frontier, verifies its canonical tree digest and asset references, writes a non-current plain-source artifact, then attaches it without creating a source revision. Equivalent archive bytes continue to share content-addressed storage.
- Explicit checkpoints are distinct events even when their canonical trees match; only request-identity replay suppresses duplication, so independent labels and authorship coexist. A checkpoint additionally verifies that the installed Loro frontier is exactly the retained frontier for its claimed source revision before committing metadata.
- Cold creation archives are committed as a fenced initial Loro source revision before a room becomes reachable. This keeps the archive a seed rather than an alternate authoritative state and lets the first checkpoint name a real retained revision.
- `Room.state`, `RoomState`, `Session`, and `Peer` are no longer public. Server socket membership/source-format/tree reads use bounded Room methods. Registry-issued `RoomHandle`s explicitly account for active requests, so eviction no longer treats incidental `Arc::strong_count` as activity. Comment projection compare-and-swap and source-anchor capture moved from HTTP/WebSocket gates into the Room command boundary. Whole-directory uploads now perform slow immutable asset staging before entering `commit_edit`, which rebuilds their merge tree from the fresh locked candidate; no server adapter can lock a Room gate or mutate its state.
- Each resident room now has a bounded Tokio command-owner task and the owner component is the sole container of `RoomState`; `Room` no longer has a state field. It admits exactly one semantic mutation lease at a time; cancelled queued requests cannot stall the owner. Source updates, HTTP/import edits, proposal decisions, annotation commands, checkpoint creation, restore, asset attachment, account-erasure cache scrubbing, and deletion pass this admission point. Restore retains one lease across the superseded checkpoint, source replacement, and restored checkpoint, and agent patch/checkpoint/comment work likewise remains one admitted command. Subscriber delivery, rate counters, and load-time construction use the owner's private state accessor but are not semantic document writers.
- The former `session_write`, `checkpoint_write`, `restore_write`, `manifest_write`, `comment_write`, and test-only bundle checkpoint gates were deleted. They no longer encode a second ordering model beside the owner. Slow immutable asset hashing/blob transfer remains deliberately outside the queue; its fenced relational attachment and cache publication enter as one command.
- Source format and main path are projections committed in the same fenced transaction as their source revision for both whole-project uploads and live browser changes. The prior post-commit in-memory-only format setter was deleted. The PostgreSQL source contract verifies the paired projection update.
- Path normalization now computes assignments by stable file identity before writing corrections, reserves every pre-existing destination before allocating collision suffixes, and is idempotent for concurrent same-path creation with an existing suffixed destination. Text and asset paths share one NFC/case-insensitive collision namespace in the native validator. Because assets do not yet have move identities, an invalid or colliding asset path refuses the candidate with recovery detail; normalization never deletes the asset reference to conceal the collision.
- The giant optional-field socket `Message` was deleted. Incoming JSON now dispatches by its `type` tag into concrete document, proposal, annotation, chat, or revision payload variants. Fields from another operation are ignored rather than entering a parallel universal model, malformed fields on a recognized tag reject deserialization, and unknown future tags retain only correlation fields for an explicit error. Multipart assembly constructs a typed `doc-update` frame rather than mutating an envelope's discriminator and payload fields.
- Empty projects are valid. Removing main selects the remaining text in deterministic path/ID order or unsets main; adding the first text selects it. No repair fabricates content.
- Native validation treats documents without an explicit marker as schema version 1, refuses malformed markers, and returns a typed upgrade-required refusal for future versions. Cold recovery detects a future schema before any legacy startup repair, preserves its exact graph for reading/export, and fences editing until the deployment is upgraded.
- `pending_attachments` and its durable write-behind path were deleted. Original anchor evidence remains durable; cold/warm equivalence coverage predates this change, but the source-revision backfill/audit remains open.

## Deletion ledger

| Responsibility | Status |
| --- | --- |
| Relay-before-commit for normal browser source updates | Deleted from active socket path |
| Normal typing dirty/save-floor state | No longer entered; shared legacy fields remain for unmigrated mutations |
| Timer, disconnect, checkpoint, and shutdown autosave calls | Deleted from production paths; the former writer is test-only migration coverage |
| Remote-writer sweep reconciliation | Deleted; the deployment lease and epoch are the sole-writer boundary |
| Mutable `pending_attachments` and `flush_attachments` | Deleted |
| `record_attachments` and annotation live-state writes | Deleted; legacy cache rows remain read-only during migration |
| Slug/creation-time identity for newly verified caches | Replaced by immutable UUID; verified legacy migration retained |
| Deployment writer-lock comment without implementation | Replaced by advisory lease plus epoch fencing |
| Separate proposal hunk/resolution/source repository writes | Deleted; one fenced proposal-decision transaction |
| Public `Room.state`, scattered adapter gates, compatibility `get`, remote-writer reconciliation, rollback coordination | Public/direct room state, adapter gates, infallible `RoomSet::get`, remote reconciliation, both rollback writers, and the six superseded internal ordering gates deleted; one bounded owner contains state and admits semantic mutations |
| Giant optional socket `Message` | Deleted; tagged payload variants deserialize directly and preserve unknown-frame correlation |
| Duplicate browser directory/review helpers | Retained where platform-specific; protocol compatibility replaces WASM unification |

## Validation performed

- `cargo test --workspace --no-fail-fast`: 368 application library tests and 3 core tests passed, 39 ignored, and all binary tests passed. A subsequent core schema test brings the focused core count to 4; the final full-suite count will be refreshed after the completion audit. Strict Clippy passed for all workspace targets and features. The earlier unrelated MCP cancellation ordering flake also passed in this complete rerun.
- Native `librepaper-document-core` workspace compilation and tests: passed. WASM compilation was removed by architecture decision.
- Disposable PostgreSQL 17: `postgres_v3_contract`, `writer_epoch_receipts_and_source_commit_are_fenced_and_atomic`, `proposal_decisions_are_atomic_idempotent_and_reauthorized`, `annotation_batch_rolls_back_as_one_receipted_commit`, `asset_registration_is_reauthorized_and_epoch_fenced`, all five proposal room round trips, all eight comment-anchor tests, the affected incremental persistence test, and `startup_repair_is_committed_before_the_room_is_served` passed.
- Disposable PostgreSQL 17: `checkpoint_metadata_commits_before_its_archive_and_replays_once` passed, proving metadata-before-artifact ordering, revision/digest reconstruction, archive attachment, and receipt replay without a duplicate row or job.
- Disposable PostgreSQL 17: all ten room persistence/checkpoint/restore tests, all eight cold/warm comment-anchor tests, all five proposal round trips, all four fenced commit contracts, and the asynchronous checkpoint-worker test passed after the command-owner and transactional source-identity cutovers. The restore suite materializes the selected asynchronous artifact before restore and verifies proposal supersession plus the four distinct timeline events.
- Focused browser checks: `file-manager.mjs`, `reader-races.mjs`, `offline-projects.mjs`, and `project-session.mjs` passed.
- The broad `npm run check` had one unrelated baseline/tooling failure because the checked-out CLI has no `local restart` subcommand. Its two architecture-related failures were corrected and the focused checks passed.

## Migration and rollback boundary

Migration `0003_document_architecture.sql` is additive. Before any new source commit writes revision/receipt rows, the prior binary can still read the existing tables. After new source writes, downgrade is not declared safe because the old binary can bypass writer fencing and does not maintain commit/source revision metadata; it must refuse deployment by operator policy. Roll forward with the new binary or restore a verified pre-cutover database/object backup. Browser UUID migration moves a verified legacy cache transactionally and preserves its CRDT operation identities.

## Open completion criteria

The following specification criteria remain unmet and must not be represented as complete: the large-state socket reference still resolves the room's then-current state instead of immutable baseline bytes and must be replaced as part of the coherent-handshake audit; representative release benchmark and provider-cost acceptance; plus production backup/restore/cutover/downgrade rehearsal. The latter items require deployment inputs or external operational authority that this implementation task does not provide; no production cutover is authorized by this ledger.
