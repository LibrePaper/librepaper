# Rust simplification: implementation shortlist

Re-evaluated against the working tree on 2026-09-21. This document contains
only the changes recommended for implementation, with their scope and required
behavior checks. No Rust changes have been made as part of this review.
Paths below are relative to `crates/librepaper/src/` unless stated otherwise;
symbol names are the stable references.

The useful work is to fix observable defects, remove obsolete
production paths, and consolidate logic whose copies must agree. Implement
these as separate changes, not as a server-wide rewrite. Line-count savings
and implementation-time estimates have been removed: neither establishes
that a change is worthwhile.

## 1. Behavior fixes

### Repair MCP suggestion-action validation

In `server/mcp/comments.rs::validate_existing_action`, reject and non-editor
refine depend on `Comment.proposed` being populated. `room/comments.rs`
constructs served comments with `proposed: None` and an empty `outcome`, so
these predicates reject legitimate actions.

Validate against the actual proposal and its state, using the existing
command authorization, version and pending-state checks. Remove predicates
that consult unpopulated presentation fields. Keep the browser-facing fields
out of this fix: `CommentCard.svelte`, `suggestions.js` and other consumers
still reference them. Exercise a real stored proposal through MCP, covering
editor rejection, author refinement, stale versions and unauthorized callers;
synthetic comments with `proposed: Some(..)` miss this defect.

### Preserve error status and hide internal storage details

`server/documents.rs::handle_comments` expects a `status` in `apply_from`'s
error value, but `read_error_value` and `command_error_value` do not supply
one. Errors from that path therefore fall back to 400, including retryable
storage errors and conflicts. Preserve typed error classification until the
HTTP response or socket frame is constructed.

Use `WriteError::client_message()` and its status/retryability methods for
room-open failures in `server/documents.rs` and `server/history.rs`.
`command_reply`, `command_error_value` and MCP's `command_failure` also expose
storage errors through `to_string()`; log those details server-side and send
a safe message. Share the classification, while retaining each transport's
wire envelope and conflict metadata. Test 409/503 responses, retry advice,
stale-selection digest fields and absence of internal error context.

### Flush buffered work when per-connection authorization is revoked

`server/socket.rs::reauthorize` flushes with `AuthorityRevoked` before leaving
the room; `reauthorize_connection` disconnects through a path that only calls
`leave`. With other subscribers still present, this does not provide the
same durability behavior for the revoked author's buffered work.

Share the revocation handling between the two paths. Preserve immediate
forced closure, room departure and the separate chat-channel behavior. Test
revocation detected by an inbound frame while another subscriber remains,
including a failed flush and subsequent retry behavior.

### Create journal temporary files with private permissions

`assistant/journal.rs::publish_durable_private` writes bytes before setting
0600. Open a new temporary file with private permissions before writing, and
preserve the file sync, rename, parent-directory sync and journal lock.
The existing `private_files::publish` is the starting point, subject to the
platform and durability requirements in section 4. Check permissions at
creation and recovery after a failed publication.

### Complete account erasure through the background worker

`server/routes.rs` exposes `/api/account/erase` and calls
`begin_account_erasure`. This is live functionality. The method marks accounts
as `erasing` and owned documents as `deleting`, but
`storage/postgres/repository.rs::finish_account_erasure` has no production
caller and `storage/worker.rs::Task` has no account-completion work.

Add bounded, restart-recoverable discovery and completion of erasing accounts,
including accounts with no documents. Wake deletion work after the request;
finish only after owned documents are gone, preserving the existing deletion
grace and attribution anonymization. Test restart recovery, repeat requests,
failure/retry, zero-document accounts and completion after the last document.
This is a lifecycle fix, not a small dead-code deletion.

### Test the CSP actually served

`server/routes.rs::document_policy` exists only under `#[cfg(test)]`.
Its assertions cannot detect changes to the headers emitted by `serve_shell`
and `serve_viewer`, whose script policies differ from that test helper.

Replace the disconnected helper tests with checks of actual response headers
for both routes. Preserve the current production policy in this change;
changing allowed script origins requires checking the renderer's loading
requirements and is outside this simplification task.

### Record the actual Quarto start time

`local/quarto.rs::timestamp_from(_started)` ignores the start instant and
returns the current wall-clock time. Consequently, `started_at` describes
collection time rather than job start; it is not necessarily byte-for-byte
equal to `completed_at`.

Capture a wall-clock timestamp when execution starts and carry it into bundle
provenance. Keep `Instant` for deadlines and elapsed time. Check that a delayed
job retains its captured start timestamp and a later completion timestamp.

## 2. Focused structural changes

### Reuse request identity and common read authorization

The middleware already stores authentication in `RequestContext`; the account
erasure handler reads it, while many document handlers call `viewer()` and
repeat authentication. Let ordinary handler identity resolution consume that
context. Preserve authentication failures as 401/503 rather than converting
them into anonymous identities.

Extend the existing `entry_viewer` pattern for repeated document-read gates.
Keep origin checks, missing/private-document behavior and route-specific role
requirements explicit. Retain mutation-time, MCP and socket reauthorization:
a successful middleware check does not make later `auth_failed` checks dead
while those paths still perform fresh lookups.

Centralize repeated authentication/error response construction beside the
existing reply helpers. Do not require one universal handler signature,
`need_room` flag or generic refusal payload for every route. Verify account,
visitor, share-link and automation behavior, including revoked credentials.

### Finish routing the remaining inline API handlers

Move the account-erasure, list and document-detail handlers from
`server/routes.rs::dispatch` into the existing `api_router` structure. This
removes the second location for ordinary API dispatch and its authentication
gates. Preserve methods, paths, origin checks, docs-host restrictions and
method-error behavior. Use the existing typed axum wrappers; keep
`DocumentService` as the store/rooms/worker boundary.

### Use one validated local build model

`local/protocol.rs` advertises only protocol 2, but
`local/service.rs::handle_jobs_post` converts `BuildRequestV2` into the old
`JobRequest` with optional mirrors of required fields, then validates those
mirrors again. Use an internal validated request with required fields and
explicit Quarto/native inputs; have the queue and runners consume it.
Apply the same approach to preview decoding. Reject unsupported versions at
the wire boundary and remove the unreachable v1 dispatch fallback and
`JobAdapter` once no admitted or restored job needs them.

Preserve scope checks before local reads, manifest verification, builder
option validation, source identity and execution-time binding checks. The
native runner still uses engine options from presets: remove obsolete fields
only when their live meaning has moved into the new model. Combine identical
terminal-failure construction here, preserving snapshot, generation and
provenance fields. Verify supported builds/previews, rejected versions,
queued jobs, cancellation and persisted job-state handling.

### Consolidate binding checks and bounded tool probes

`local/preview/quarto.rs` and `local/preview/calepin.rs` repeat scoped binding,
canonical-root and entrypoint checks. Extract those common checks, leaving
Quarto's declared inputs, frozen policy and source verification in its adapter.
Use the same checks in build admission and immediately before execution;
sharing their implementation must not eliminate checks across a queue delay.

Share bounded executable/version probing among `local/discovery.rs`,
`local/quarto.rs` and `local/preview/calepin.rs`, retaining tool overrides,
search precedence, output limits, timeout and process cleanup. Pass the
resolved Calepin executable to its command plan instead of discovering it
again. Preserve preset grants and check the admitted semantic revision before
spawn for preset-backed builders. Test binding replacement/revocation, preset
changes during queueing and hung probes.

### Remove obsolete direct-TeX builder code

`local/builders/mod.rs::BuilderId::parse` accepts Typst, Pandoc and Calepin;
Quarto has its own dispatch. Remove direct `tex`/`latexmk`/`tectonic` acceptance
from request validation, unreachable native-runner and diagnostic branches,
and `local/texlog.rs` after its dead caller is removed. Retire TeX-only pass
counters and discovery metadata that no supported builder consumes.

Keep browser LaTeX support and Quarto's PDF tooling. Keep the live tool-search
configuration: despite its name, `--tex-path` currently supplies search
directories for Typst, Pandoc and Calepin. `Capabilities.tools` and
`Confinement` also have live settings-UI consumers and are outside this cull.
Validate supported builder discovery, options, diagnostics and outputs, plus
explicit rejection of direct-TeX requests.

### Use the canonical projection for file reads and comment attachment

`document/session` schema readers and `librepaper_document_core::project`
disagree on path collisions. Migrate `room/resolve.rs::Sources::of`, anchor
lookup and `document/store.rs::project_files` to the canonical projected file
set, retaining stable file IDs and the CRDT cursor APIs. Import the shared
root constants instead of redeclaring them.

Avoid recomputing the full projection on every comment page. Provide head
and cached projection through one sequencer callback, for example
`with_projected_head(|doc, projected| ..)`, under the same lock. Resolve
cursors and read the digest inside that callback: independent `projection()`
and `with_head()` calls permit mixed revisions. Keep the document-core crate.
Test colliding/invalid paths, assets, renamed files, cursor resolution and
attachment/digest consistency across concurrent edits.

### Reduce comment-write plumbing and repeated event reads

Group command caller data into a small context built from the existing
`Viewer` authority/authorization methods, reducing long positional
constructors. Keep attribution, session generation, policy ceiling and
link-bounded authority distinct; do not introduce another principal model.
Parse comment UUIDs at transport boundaries and carry them through internal
command dispatch.

In `room/comments.rs`, event enrichment rereads a comment and collection state
for editor broadcast, reader broadcast and the initiating caller. Prepare the
required comment data once per mutation and derive the viewer-specific forms
from it. Preserve attachment/reply enrichment and reader redaction; collection
counts describe the whole collection and cannot be inferred from one returned
comment. Use a typed internal event where it removes JSON inspection, while
keeping the wire shape. Check that query counts no longer grow per audience
and that suggestions and private anchors never reach readers.

For resolve/unresolve, replace whole-row reconstruction with an authorized
update returning the needed row. Keep proposal accept/reject/refine as full
semantic commands: their work is not merely changing one boolean.

### Narrow visibility and delete demonstrated unused production APIs

`lib.rs` exposes whole modules for integration tests and fuzz targets. Narrow
individual items or use explicit exports so production-only dead code becomes
visible to the compiler. Preserve the APIs actually used by
`crates/librepaper/tests`, `tools/fuzz` and benchmarks, not just unit tests.

Repository searches found no callers for `documents_by_owner`,
`documents_by_owner_page`, `check_storage_admission`, `asset_sizes_by_digests`,
`replace_suggestion_authorized`, `supersede_proposals`, `forget_marks` and
`apply_annotation_batch`. Remove these methods and their exclusively owned
support types. Also remove the uncalled `engine_adapter::quarto_shared_paths`,
session `Presence`, and unused `AnchorStatus::parse` and
`ResolutionDiagnostic::parse` methods. Drop the stale module-level dead-code
allowance in `document/hunks.rs` and address any actual warnings it reveals.

Keep test/benchmark support such as `Sequencer::is_warm`, `drop_cache`,
`subscribers`, catalogue fixture writers and annotation-count assertions;
restrict their compilation/visibility where practical. The old session
admission path is exercised by `tools/fuzz/fuzz_targets/update.rs`: replace
that target with coverage of the live admission/import path before retiring
the obsolete implementation. Likewise preserve or migrate fuzz coverage using
`session::replace_text` and `text_of`. Compile the integration, benchmark and
fuzz targets affected by these changes.

## 3. Local refactors with a concrete payoff

- **Share label creation.** Replace `server/history.rs::Label` with the
  `room/label.rs::TakeLabel` path, including its replay lookup. Preserve
  attribution, request IDs, reason and the Quarto checkpoint's comparison
  against the digest actually captured by the label. Check duplicate requests
  and concurrent edits during checkpoint creation.
- **Avoid discarded patch copies.** Extract validation from
  `room/agent.rs::apply_patches` for `AgentPatchCommand::evaluate`, which
  discards the cloned result. Retain result construction for MCP candidate
  creation, which consumes `AppliedSource`. Keep validation under the
  sequencer lock and preserve range, identity, dependency, overlap and final
  size checks. Use `apply_edits_at` where `apply_patch_edits` discards the
  per-file diff and `Head::prepare` already exports the command diff.
- **Share transaction-local queries.** Factor identical annotation reads
  and reply-summary queries into executor-generic internal helpers. Retain
  transaction-taking entry points so commands read their uncommitted writes
  without acquiring a second pool connection. Extract the repeated writer
  epoch check into a helper operating on the caller's transaction, preserving
  the writer/document/authorization lock order and scratch accounting.
- **Reuse the superseded-base deadline query.** The worker's sweep and the
  catalogue's pending-work scan both query the next snapshot deletion
  deadline. Give that query one catalogue implementation, preserving bounded
  sweeps and rescheduling. Do not move all maintenance SQL merely for location.
- **Extract exact sequencer repetition.** Share identical subscriber fan-out
  and fence/close code and repeated readiness checks where lock ownership and
  failure behavior match. Keep `fenced` and `unreadable` distinct, retain lazy
  cache warming and existing queue-pressure behavior. This does not require
  a new health state machine or changing memory-pool ownership.
- **Simplify the store's internal API.** Remove `get_checked` in favor of
  `get_result`; make the infallible, non-awaiting `open_with_catalog`
  constructor synchronous and infallible; remove the ignored owner-key
  parameter from `owned_by`. Preserve account-based ownership checks.
  Leave serialized `IndexEntry` fields out of this cleanup.
- **Trim the catalogue test seam.** Remove unused `log_head` and `log_base`
  methods from `LogCatalog`: admission calls the concrete catalogue directly.
  Keep the trait's production methods and explicit future/lifetime signatures.
- **Share test setup that really is identical.** Centralize schema reset SQL
  and repeated deployment construction within the unit-test and integration-
  test suites. Preserve database opt-ins, serialization locks, writer leases,
  resource lifetimes and scenario-specific configuration. A shared reset
  list must not introduce concurrent truncation of another test's database.
- **Split the local modules along existing responsibilities.** After the
  request-model cleanup, extract job queue/admission and pairing handlers from
  `local/service.rs` using the existing `&Inner`/`pub(super)` pattern. Separate
  binding persistence and bundle collection from `local/quarto.rs` execution.
  Keep this mechanical: no routing rewrite, package merge or fingerprint
  change. The local computation-fingerprint function already delegates to
  the parser's implementation.

## 4. Small cleanups worth retaining

- Replace the handwritten `local/quarto.rs::md5_hex` with a direct dependency
  on the appropriate MD5 crate. Lockfile presence alone is not a usable direct
  dependency. Preserve Quarto freezer compatibility with known digest vectors
  and existing frozen-cache tests; MD5 remains a format requirement here.
- Share the equal-length byte comparison used by pairing tokens and service
  codes. Keep trimming in the code-specific caller and digest parsing/length
  checks at their existing boundaries.
- Consolidate equivalent private-file replacement writers, starting with the
  journal and CLI token writer. Preserve relative-path handling, private
  creation, temporary-file cleanup, file-handle closure before rename and
  platform-specific directory syncing. Backup files and auth keys currently
  use create-only semantics; do not route them through an overwriting rename.
  Keep caller locking and durability requirements explicit.
- Reuse named SHA-256 hex helpers when their inputs and encoding are identical.
  Preserve domain prefixes, framing and persisted fingerprint bytes. This is
  local cleanup, not a requirement to replace every inline hash expression.
- Remove `_tracked_file_count = inventory.files.len()` from Quarto collection.
  Keep `_execution_inventory_before`'s verification until a validation-only
  pass preserves `read_verified_manifest` checks over the effective inputs at
  the same execution stage; only its unused aggregate digest is expendable.
- Remove the unused `Search.files` field in `room/locate.rs`. Keep initial
  rendered-selection matching and existing-comment reattachment algorithms
  separate: they operate on different evidence and character representations.

## Implementation order

1. Land the behavior fixes independently, with focused regression coverage.
   Account erasure is its own lifecycle change, not part of a mechanical sweep.
2. Narrow visibility and remove unused APIs; migrate affected fuzz coverage.
3. Simplify the local request model, then remove obsolete direct-TeX paths and
   consolidate binding/probe logic. Split the local modules afterward.
4. Consolidate request identity/error handling and finish the inline API routes
   in separate changes. Preserve commit-time authorization throughout.
5. Migrate canonical projection reads, then reduce comment-event reads and
   command plumbing. Test concurrency and audience redaction at each step.
6. Apply the local refactors and small cleanups independently as their modules
   are touched. No blanket error-library migration, newtype rollout or
   repository-wide helper sweep is required.
