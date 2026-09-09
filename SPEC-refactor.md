# Deferred room and storage refactors

Status: proposed umbrella roadmap; implementation is not part of this document.

Implementation contracts and delivery milestones are indexed in
[the refactor implementation specs](docs/specs/refactor/README.md). Each track
inherits the guarantees below. A track is complete only when its own acceptance
criteria are met; completing one does not imply completion of this roadmap.

Baseline: `864dbb8` on `live-markdown-editor` (2026-09-08), incorporating the
room fixes at `328a45b` and storage fixes at `b87d92b`. Concurrent, uncommitted
document and web changes are outside this baseline. Reconcile those changes
before implementing overlapping work. Storage findings and their dispositions
are recorded in [the combined storage review](docs/reviews/storage-review.md).

## Purpose and boundaries

Reduce editing stalls, repeated storage work, and inconsistent internal APIs in
`crates/librepaper/src/room/` and `crates/librepaper/src/storage/`, and close the
remaining attribution and storage-limit gaps. Preserve the durability,
authorization, quota, and concurrency guarantees established by the fixes.
Deliver this work as separate, reviewable changes rather than another
wholesale module rewrite.

The following correctness work is already complete and is not a new task here:

- Checkpoint acknowledgement delivery, checkpoint/session generation handling,
  duplicate checkpoint handling, negative history allowances, and full catalogue
  retention beyond the resident history tail.
- Publication reservation refunds and pending-publication label handling.
- Room loading, eviction, and deletion fencing; lease renewal refusal when its
  safe interval has expired; automatic-checkpoint scheduling progress.
- Suggestion decision permissions, rejection of oversized proposals and source
  anchors, durable acceptance retries, and rollback/broadcast regressions.
- Pending-edit quota accounting and asset upload/pruning races.
- Catalogue rendering lookup beyond the resident tail and releasing the room
  state mutex during rendering uploads.
- Targeted catalogue updates for comment decisions, cached CRDT encoded size,
  removal of duplicate session recovery, and delegation of `format_from_path`
  to the existing document format detector.
- Journal failure reconciliation, binary recovery bases with legacy decoding,
  exact segment framing, erasure stage cursors, manifest retirement, shared
  segment ownership transfer, and synchronization of recovery and reclamation.
- Rendering retirement and authorization checks, publication withdrawal,
  maintenance accounting, filesystem temporary-object filtering, private
  backup staging, canonical manifest verification, and native S3 pagination.

Ordinary comment resolution remains available under the existing commenter
policy. Suggestion decisions require editor rights. Do not reinterpret the
original review as a request to make all comment resolution owner-only.

## Guarantees every refactor must preserve

- A `y-ack` acknowledges only a durably saved update, including updates saved
  through checkpoint and compensation paths.
- Retrying a prepared suggestion acceptance replays its stored CRDT state and
  cannot apply the proposal twice. Failed operations preserve unrelated edits
  and keep connected peers convergent.
- Deletion and loss of write authority fence subsequent durable mutations.
  Moving validation before an asynchronous wait must not introduce a window
  in which stale authorization permits a write.
- Room admission includes pending and in-progress writes. Cancellation and
  failure release reservations according to their existing ownership rules.
- Eviction cannot create a second writable instance while a request or
  checkpoint still owns the first instance.
- Pruning uses complete retained history, protects live and in-flight content,
  respects rendering grace periods, and skips unsafe deletion when its input
  cannot be read. The resident manifest is only a bounded cache.
- Event identity and tree content identity remain distinct: two history events
  can refer to the same content without becoming the same event.
- Existing persisted records, request digests, wire fields, and client
  correlation IDs remain compatible unless a change explicitly supplies a
  migration or compatibility adapter.
- Physical reclamation releases accounting only after deletion is confirmed.
  Ambiguous writes retain conservative charges and durable recovery or cleanup
  records; a transient read error is not evidence that an object is absent.
- The catalogue-owned journal gate protects the current single-process local
  authority. Preserve it across graph reads, publication, recovery, and
  reclamation. It does not establish a cross-process or hosted reader protocol;
  adding such support requires a separate fencing and lease design.

## 1. Move catalogue work off Tokio workers

Priority: high. Primary files: `storage/catalog/` and asynchronous callers in
`room/`, `storage/journal/`, maintenance, server handlers, and CLI paths.

The catalogue uses `std::sync::Mutex<rusqlite::Connection>`. Room operations call
its synchronous methods from asynchronous tasks. Connection lock contention,
SQLite busy waits, and long queries can block a Tokio worker; holding room state
while making those calls also blocks that document's editing operations.

Introduce one asynchronous catalogue execution boundary with bounded admission.
A dedicated database worker with a bounded request queue is the preferred
starting point. A bounded `spawn_blocking` adapter is acceptable if it offers
equivalent transaction ownership, shutdown, and cancellation behavior. Keep
synchronous SQL and transaction helpers together behind that boundary.

Requirements:

- Submit a whole transaction as one operation. Never split an existing atomic
  receipt, quota reservation, checkpoint insertion, or object-accounting change
  across separately queued statements.
- Carry owned inputs and results across the boundary. Do not carry room mutex
  guards, borrowed SQLite transactions, or live Yjs transactions into it.
- Bound queued work and define backpressure. An unbounded collection of
  blocking tasks waiting for one connection is not an acceptable replacement.
  Bound retained input bytes, executing work, and waiting producers as well as
  request count. Acquire memory admission before constructing or copying large
  owned inputs; waiting outside a bounded channel must not bypass the budget.
- Define cancellation explicitly: cancelling a caller does not undo an SQL
  operation that already started. Use the existing operation IDs and receipts
  to recover uncertain outcomes instead of blindly repeating mutations.
  Assign ownership through queued, executing, committed, and reconciled states.
  A service-owned completion path must settle temporary edit reservations and
  other cleanup even when the caller disappears. Inventory operations without
  receipts and give each a reconciliation rule before migrating it.
- Keep process-local room edit reservations on the same connection and within
  the same accounting model. A connection pool must not silently fragment the
  temporary reservation table.
- Preserve the deployment writer lock. This change does not introduce support
  for multiple active writers to one deployment.
- Drain or reject queued work predictably during shutdown, and expose queue
  wait and SQL execution duration separately for diagnosis.

Acceptance: a deliberately blocked catalogue operation does not stop a Tokio
heartbeat or unrelated socket work that does not require catalogue admission.
An edit requiring a catalogue quota reservation may wait; moving SQL to a worker
does not remove that dependency or authorize broadcasting an unreserved edit.
After section 2, that wait must not retain room state or the global registry.
Queue saturation and producer memory are bounded. Cancellation, shutdown, and transaction-failure tests preserve quota
and receipt invariants. Audit asynchronous call sites so direct synchronous
catalogue calls do not remain on request paths.

## 2. Shorten remaining room and registry lock scopes

Priority: high. Primary files: `room/comments.rs`, `room/mod.rs`, and catalogue
callers throughout `room/`.

Rendering uploads already release the room state mutex. Legacy comment writes
still call `save(&mut RoomState)` while holding it, including lease and blob
storage work. Catalogue calls also remain under that mutex in several paths.
Room admission holds the global rooms registry while awaiting per-room state.

Separate preparation, persistence, and in-memory completion:

1. Validate and prepare an owned mutation or immutable snapshot under state.
2. Release state before waiting on storage or catalogue execution.
3. Reacquire state to complete the operation, checking the relevant version,
   generation, and write fence before changing shared state.

Use a dedicated operation gate where ordering requires one. For legacy comment
snapshots, preserve conditional-write versions and serialize comment writers
without blocking source edits. A failed write must not replace a newer state
snapshot. Broadcast and acknowledgement ordering must follow the operation's
actual persistence semantics.

Document the lock order for `restore_write`, `checkpoint_write`,
`manifest_write`, `session_write`, `publication_write`, `rendering_write`, `assets_write`, room state,
and the rooms/loading registries.
Changing a lock scope must account for deletion, cancellation, and pruning; it
is not sufficient to move an `.await` outside braces mechanically.

For admission, capture candidates without retaining the global registry guard
across expensive per-room work, then revalidate membership and eviction
eligibility before removing an entry. Preserve the existing protection for
active `Arc<Room>` owners; temporary references used by admission must not make
every candidate appear permanently busy.

Acceptance: paused legacy comment persistence allows source edits and socket
activity to proceed. Concurrent comment writes cannot overwrite one another.
A slow room does not prevent an unrelated cached room from being retrieved.
Cancellation, purge/load, and active-request eviction regressions remain green.

## 3. Share history traversal and tree loading during pruning

Priority: medium. Primary files: `room/checkpoint.rs`, `room/figures.rs`, and
`room/catalog.rs`.

`prune_assets`, `prune_renderings`, and `prune_blobs` independently obtain
complete history. Asset and text pruning separately load retained trees.
Rendering pruning primarily needs checkpoint identities and labels; it should
reuse that metadata without being forced to fetch tree bodies.

Build a pass-scoped retention plan from one consistent, paginated view of
retained history. Resolve each unique tree at most once and accumulate the
referenced text and asset digests, labelled rendering identities, and newest
retained rendering. Release tree bodies after extracting their references;
avoid an unbounded cache of decoded history.

Use bounded concurrency for independent tree reads. Tie the plan to the
history version or existing manifest serialization, and revalidate changes
that could make deletion unsafe before deleting. Continue protecting live
document references, current checkpoint output, active uploads, recent
renderings, and changes made after the initial scan. Do not replace the final
asset deletion checks with a stale shared snapshot.

Acceptance: instrumented storage demonstrates one history traversal per pass
and at most one tree read per unique retained tree. Tests cover more than 64
checkpoints, shared content identities, concurrent labels/restores/uploads,
missing trees, and failures between manifest persistence and deletion. An
unreadable dependency prevents deletion of objects it might reference.

## 4. Avoid redundant rendering queries and downloads

Priority: medium. Primary files: `room/figures.rs`, `storage/catalog/checkpoints.rs`,
`storage/blob.rs`, and rendering request handlers.

Catalogue-mode `has_rendering` downloads the object to check availability.
Callers that subsequently read it can download the same PDF twice.
`newest_rendering` also loads history and performs a rendering query for each
candidate, then computes the current tree digest.

Provide a catalogue query or bounded joined page that finds rendering
candidates in history order, including checkpoints outside the resident tail.
Preserve the distinction between event SHA and content SHA.

For requests that need bytes, resolve and read once, returning the bytes with
the resolved metadata. For metadata-only availability checks, consider a blob
metadata/existence operation with proper local and remote implementations.
Catalogue registration alone does not prove that a blob exists. Distinguish
missing objects from transient storage errors, and retain the caller's
specified fallback behavior when a candidate is unavailable.

Cache the current tree digest only if profiling warrants it. Any such cache
must invalidate for text, path, main-file, and asset-reference mutations,
including restore and generic Yjs updates; session durability alone is not an
invalidation signal.

Acceptance: serving a PDF performs at most one body download for that object.
Metadata lookup performs no PDF body download when the backend supports a
metadata operation. Query counts do not grow one-for-one with skipped history
entries. Old retained PDFs, missing objects, transient failures, and restore
events sharing content retain correct behavior.

## 5. Complete resident-size caching

Priority: medium, guided by profiling. Primary file: `room/mod.rs`.

The committed fixes cache encoded CRDT size by generation and remove catalogue
queries from `resident_bytes`. Comments and manifest metadata are still
serialized for each estimate; admission still scans resident rooms.

Cache component estimates with explicit invalidation for comment/reply changes,
manifest changes, assets, renderings, and source generations. Consider a running
registry total only after defining how updates, admission, replacement, and
eviction publish accounting changes without undercounting or double counting.
Avoid a state-to-registry lock dependency that reverses admission's lock order.

The estimate is an admission measure, not an exact measurement of allocator
usage. Preserve conservative, saturating arithmetic and document what it
includes. Never reuse storage quota accounting as a substitute for memory
accounting.

Acceptance: repeated admission checks against unchanged rooms perform no CRDT
encoding or comment/manifest serialization. Every relevant mutation updates or
invalidates its component. Byte-ceiling and active-room admission tests retain
their behavior, including overflow and failed-estimate handling.

## 6. Make write refusal and error handling explicit

Priority: medium. Primary files: `room/mod.rs`, `room/checkpoint.rs`,
`room/figures.rs`, and their server/CLI callers.

Several mutators, including `set_main_file`, `add_text`, and `name_asset`, still
signal write refusal through an empty update or no return value. Other paths
return strings, and rendering handlers classify quota errors by substring.

Introduce typed room write errors covering the distinctions callers actually
need: read-only/fenced, permission denied, quota exceeded, not found, conflict
or stale input, invalid input, and storage failure. Preserve underlying error
context for logging without exposing internal details to clients.

Convert silent mutators to explicit `Result` contracts and update publication,
restore, rendering, and CLI callers to stop at the first refused operation.
Keep a valid empty CRDT update distinguishable from a refused mutation.
Centralize HTTP and socket error mapping, retaining request and temporary IDs.

Do not replace durable catalogue status checks or legacy lease checks with a
single cached `read_only` boolean. A common API should expose these guarantees,
not weaken them. Preserve existing `AcceptError` distinctions where useful.

Acceptance: read-only writes fail before dependent publication work proceeds;
quota failures retain their intended HTTP status; handlers classify errors
without inspecting message text. Changing an error's wording cannot change
its status, retry policy, or cleanup behavior.

## 7. Validate commands before operation-specific processing

Priority: medium. Primary files: `room/mod.rs`, `room/comments.rs`, and
`server/socket.rs`.

The wire `Message` is a string discriminator plus many defaulted fields. Unknown
comment operations can enter rate accounting and fall through to an unrelated
body-validation error.

Keep a compatible wire adapter, then convert to validated internal command
variants with only the fields each command needs. Reject unknown kinds with a
specific protocol error before consuming the comment mutation allowance.
Retain connection/frame abuse controls for malformed traffic so this ordering
does not create an unmetered expensive parsing path. Preserve the existing
idempotent retry behavior and correlation fields.

Acceptance: representative existing client frames still decode; missing and
invalid fields produce operation-specific errors; unknown kinds do not consume
the comment mutation allowance; oversized frames remain bounded; successful
retries do not charge or mutate twice.

## 8. Consolidate small shared policies and helpers

Priority: low. Keep these changes separate from the concurrency work.

| Area | Required scope and compatibility checks |
| --- | --- |
| Checkpoint content identity | Add a shared accessor for `tree_sha` with legacy `sha` fallback, including catalogue row conversion. Replace repeated rendering/retention expressions without changing event identity comparisons. |
| Default main-file names | Move format-to-default-path policy beside the shared format definition. `format_from_path` already delegates to the detector; do not claim that duplication still needs fixing. Preserve explicit names and unknown-format fallback. |
| Comment views | Use one conversion helper for author/owner-dependent `CommentView` construction. Preserve privacy and per-viewer edit/delete flags across snapshots and events. |
| Rendering publication | Extract the common reservation, upload, registration, and cleanup sequence from the rendering entry points. Keep current-tree validation and historical-checkpoint validation distinct, with their required final checks and lock order. |
| UTF-16 operations | Inventory room, session, CLI, and text-crate helpers before consolidating. Specify checked versus clamped offsets, surrogate boundaries, overflow, and invalid ranges. Keep caller-specific behavior explicit; test non-BMP and combining characters. |
| Request digests | Reuse shared byte hashing. Simplify JSON canonicalization only after proving byte-for-byte compatibility with stored receipts, including nested key order, escapes, and numbers. Do not assume serialization order is stable under every `serde_json` feature combination; keep explicit canonicalization or version the format if necessary. |
| Bulk comment persistence | Normal decisions already update individual catalogue rows. Restrict the remaining full-snapshot reconciliation helper to legacy/import/fixture needs, document those callers, and batch queries if it remains a production path. |
| Unused fields and documentation | Audit `Peer.address` and `attach_deployment_lock_unavailable` across production callers, tests, and supported configurations before removing them. The unused renewal constant and misplaced save documentation were already corrected. |

Acceptance: public behavior and persisted identities remain unchanged, existing
focused tests pass, and new tests cover shared policy boundaries rather than
simply repeating helper implementations.

## 9. Persist stable checkpoint author identities for erasure

Priority: high. Primary files: `storage/catalog/` schema and checkpoint/erasure
operations, `room/checkpoint.rs`, and other checkpoint creation/import callers.

Checkpoint attribution currently stores display handles while erasure matches
account IDs. Renamed or reused handles cannot reliably identify an account's
historical contributions.

Persist an optional stable account ID separately from display attribution.
Obtain it from authenticated caller identity and pass it through every
checkpoint-writing path. Define anonymous, imported, and system-authored
checkpoints explicitly; a display string must never become an account ID by
assumption. Keep existing display and history behavior compatible.

Supply a schema migration and update row conversion, backup/restore, and
checkpoint callers together. Backfill historical IDs only when authoritative
records establish the association. Preserve unresolved legacy attribution as
an explicit migration limitation; do not match by current handle, erase another
account's contributions, or claim complete historical erasure without evidence.

Erasure must use the stable ID to remove identifying attribution, including
the associated display metadata, through bounded, restartable batches. Preserve
checkpoint content and event identity, and ensure new writes cannot reintroduce
attribution for an erasing account after an asynchronous wait. If attribution is
also retained in immutable objects, define their replacement and reclamation
before declaring that part of erasure complete.

Acceptance: cover account renames, reused handles, distinct accounts with the
same display name, anonymous/imported checkpoints, legacy rows, interrupted
migrations and erasure passes, and concurrent checkpoint creation. Backup and
restore preserve the identity distinction. Erasing one account does not remove
another account's attribution or change retained document content.

## 10. Align document admission with journal and recovery limits

Priority: high. Primary files: `config.rs`, publication and room admission,
`storage/journal/coordinator.rs`, `segment.rs`, and `recovery.rs`.

`--max-size` currently permits 100 MiB documents, while the default journal
queue holds 64 MiB of aggregate payload and recovery bases have a 64 MiB payload
bound. The binary codec fixes JSON expansion for ordinary documents; it does
not make all advertised size configurations durable.

Define one supported relationship among source size, encoded CRDT snapshot
size, queue capacity, segment framing, recovery-base size, and compaction peak
accounting. Include CRDT and metadata overhead: a source-byte ceiling is not an
encoded-state ceiling. Preserve bounded aggregate memory across concurrent
rooms; raising a global constant alone is insufficient.

For the first delivery, reject unsupported configurations and oversized updates
before accepting work that cannot be durably saved. Preserve existing record
chunking and persisted formats; expanding recovery capacity or introducing a
streaming persistence pipeline is a separate future proposal. The
[size-limit implementation spec](docs/specs/refactor/10-size-limits.md) defines
the source and encoded-state limits and the validation needed to set them.
Distinguish temporary queue
saturation from a permanently unsupported document size. Propagate the chosen
limits to startup/configuration validation, CLI/server errors, and documentation.
Retain existing segment and recovery compatibility or provide a versioned
migration if the persisted format must change.

Acceptance: exercise the default 4 MiB boundary, the 64 MiB boundary, the maximum
advertised configuration, and just-over-limit inputs, including CRDT overhead.
Every accepted snapshot can be appended, acknowledged, compacted, backed up,
restored, and recovered after restart. Concurrent large documents remain within
queue and memory budgets; rejected work cannot enter a repeated-save/quota-loss
loop. Verify cancellation and temporary compaction accounting at these limits.

## 11. Add bounded S3 retries and batch deletion

Priority: medium. Primary files: `storage/s3.rs`, `storage/blob.rs`, and
maintenance callers. Native listing pagination is already implemented.

Define retries by operation and failure class. Bound attempts and total elapsed
time, use backoff with jitter, honor applicable provider retry delays, and retain
cancellation. Authentication failures, invalid requests, NotFound, and
conditional-write conflicts must retain their distinct meanings. Reconcile an
ambiguous write before deciding whether to retry; never silently replace a
conditional write with an unconditional overwrite or repeat a product mutation.

Use provider-supported batch deletion with bounded batch sizes. Account for
each object's outcome, including partial failures in an otherwise successful
HTTP response. A successfully deleted or already absent object may complete its
retirement; failed or uncertain objects retain their queue rows and charges.
Adapt the blob API/callers where needed so mixed outcomes remain expressible.
Preserve create-only backup publication and current listing cursor semantics.

Acceptance: a mock provider exercises throttling, transient server failures,
connection loss after an accepted write, conditional conflicts, retry exhaustion,
cancellation, and partial deletion responses. Retries respect their bounds and
batch failures never release accounting for an object whose deletion is
unconfirmed. Local blob behavior remains compatible. This work does not enable
hosted serving or multiple deployment writers.

## 12. Make remote backup creation and cleanup mutually exclusive

Priority: medium, before exposing the currently unused blob-backup API to
concurrent callers. Primary files: `storage/backup.rs` and the blob-store contract.

Same-ID creation and incomplete-backup cleanup currently require external
serialization. A final NotFound check before deletion narrows a race but cannot
prevent a creator from publishing its manifest immediately afterward. The local
CLI backup path already has a separate offline lock; preserve that behavior.

Choose and enforce an ownership protocol covering private object writes,
completion publication, and cleanup for a backup ID. An externally held lock
may remain appropriate if the API makes that obligation enforceable. Supporting
distributed callers requires conditional claims/fencing with defined crash and
stale-owner recovery; a process-local mutex or a time-based guess is insufficient.
Do not weaken completion-marker immutability or treat transient reads as absence.

Acceptance: deterministic barriers interleave creation, cleanup, and competing
creators around the final manifest check/publication. Completed backups remain
verifiable and unchanged; abandoned attempts can be reclaimed after ownership
is safely recovered. Cover crashes and transient reads as well as successful
cleanup. Until implemented, retain the documented serialization requirement.

## External review coverage still to assess

The external storage report's content endpoint returned HTTP 403. Only the
findings included in the supplied summary were reconciled with the local review;
its remaining medium, low, and nit findings are not verified implementation
tasks or completed work.

When the full report becomes available, map each additional finding to the
current source and existing sections here. Record it as confirmed, already
fixed, duplicate, unsupported, or deferred, with evidence and an actionable
acceptance criterion where work remains. Do not carry forward the external
severity totals as independently established totals.

## Delivery and validation

First settle the size-limit policy and catalogue execution/cancellation contract
in their implementation specs. Deliver the size-limit correctness fix as the
first milestone; stable attribution is an independent early correctness
milestone and must not wait for the performance work. Catalogue execution and
lock scope changes follow in coordinated but separately reviewable steps.
Follow with shared pruning and rendering lookup,
then memory estimates and API cleanup. Small independent helper extractions can
land separately. Stable attribution and size-limit work are correctness tracks
that can proceed independently; neither should wait for performance profiling.
S3 retry/batching and remote backup ownership are separate operational changes.
Each change must include a before/after description and the specific failure or
cost it addresses, with migrations and caller updates delivered together.

Use deterministic blocking/failure fixtures and operation counters for
concurrency and I/O assertions. Avoid timing-only tests. Benchmark representative
small and large documents, rooms with many comments, histories above 64 entries,
and concurrent activity across rooms. Record queue wait, lock hold time, query
count, tree reads, and downloaded bytes where relevant; compare results on the
same build and workload rather than prescribing an unsupported latency target.

Retain the regression suites in `tests/room_checkpoint_fixes.rs`,
`tests/room_lifecycle_fixes.rs`, `tests/room_figure_fixes.rs`, and
`tests/room_suggestion_fixes.rs`. Run relevant existing room, history, admission,
quota, socket, rendering, and suggestion tests for each change, followed by
workspace tests, formatting, and Clippy with warnings denied before merging.
Also retain the storage catalogue, journal, maintenance, blob, backup, and S3
regressions, including large-snapshot recovery, erasure restart, shared-segment
deletion, ambiguous writes, quota preservation, and pagination boundaries.
The room review recorded 783 passing tests and one ignored; the storage fixes
rebased onto it recorded 808 passing tests and one ignored. These are historical
validation records, not required fixed test counts for the current branch.

Completion requires measured reductions in the targeted work, preserved
correctness under cancellation and injected failures, and a final call-site and
lock-order audit. This specification does not authorize removing legacy storage,
changing retention policy, changing comment permissions, changing wire formats,
or weakening quota/durability checks to obtain those reductions.
