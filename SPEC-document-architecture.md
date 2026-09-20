# Document ownership, durable commits, and collaboration simplification

Status: proposed implementation specification, 2026-09-19. This is an ambitious target architecture, not a description of completed work.

Architecture decision, 2026-09-19: the native/WASM unification proposal is rejected. LibrePaper must never compile the browser application or a Rust/WASM document core on the server. The browser retains its existing JavaScript `loro-crdt` integration; the extracted Rust document core is native-only and is used for authoritative server validation. This decision supersedes every native/WASM binding, browser binding prototype, and cross-platform shared-implementation requirement below. Interoperability is verified at the protocol and fixture boundary instead of by sharing a compiled implementation.

## 1. Objective

Make LibrePaper's collaborative document system understandable through three ownership rules:

1. One browser project session owns that browser's local work.
2. One server document task owns the authoritative document and all its semantic mutations.
3. One database transaction makes an accepted mutation durable before anyone else can observe it as authoritative.

The purpose is to delete coordination machinery, duplicated domain behavior, and ambiguous state transitions. Moving the same machinery into new modules does not satisfy this specification.

The target retains Loro, stable file identities, concurrent and offline editing, branch-based proposals, browser rendering, PostgreSQL, and immutable asset storage. It replaces the way those parts are coordinated. It does not introduce a generic actor framework, an application-wide event-sourcing framework, a message broker, a second document database, or a second CRDT.

The central change is that the server no longer holds accepted-but-unpersisted document state. Local optimistic work exists in clients. Server candidates exist temporarily while preparing a transaction. The document visible through authoritative server reads consists only of committed work.

## 2. Relationship to existing work

This specification follows the review of `document/session`, `room`, `project-session.js`, and the reader collaboration controller. Source comments and earlier specifications explain intent but are not evidence of implemented behavior.

Relevant existing specifications are [the Loro substrate](docs/specs/SPEC-loro.md) and [offline continuation](docs/specs/SPEC-offline-continuation.md). Preserve their product requirements except where this document explicitly changes them.

The current working tree has removed publication and rendered bundles; `TODO-storage.md` records that runtime cutover as implemented, with legacy-data cleanup and deployment validation still outstanding. Authorized viewers and commenters receive current source projections and current assets, while editor CRDT history remains separately authorized. Do not recreate publication, rendered bundles, or periodic source archives as part of this project. Working-tree implementation is not evidence of production deployment.

The publication-removal design's emphasis on low storage cost remains relevant. This specification explicitly supersedes the 30-second autosave floor and visibility-before-durability model if adopted after the early evidence gates below. Browser rendering and current-source access boundaries are preserved. Recheck the working tree before implementation; storage work is ongoing.

Comments already name Loro frontiers instead of creating full project checkpoints (`room/comments.rs`). The remaining work is to establish durable revision references and source/comment atomicity, verify recoverable original evidence, and remove any remaining durable derived caches. Do not count avoiding a checkpoint per comment as a new saving from this redesign. The current source persistence path also advances `durable_vector` only after a successful append (`room/mod.rs`); this proposal strengthens authoritative visibility semantics, rather than introducing durable acknowledgement for the first time.

The following are deliberate changes:

| Existing behavior or structure | Target |
| --- | --- |
| Apply, relay, later persist | Prepare, commit, install, broadcast |
| Several independently locked mutation paths | One document command owner |
| Active rooms merge another writer's database updates | One enforced deployment writer; recovery at ownership changes |
| Comments name live Loro frontiers without creating full checkpoints | Comments name verified committed source revisions with atomic source dependencies |
| Mutable attachment caches are routinely persisted | Immutable anchor evidence plus disposable revision-keyed caches |
| Rust and JavaScript implement document rules separately | One domain implementation with native and browser adapters |
| Socket, reader, and project layers share reconnect ownership | One project lifecycle with replaceable connections |
| Empty projects require corrective main-file creation or selection | Empty projects are valid; main selection is explicit or deterministically resolved |

### 2.1 Confidence and early evidence gates

Keep the ambitious target where it permanently removes coordination or duplicated behavior. Scope alone is not a reason to abandon it, but the benefits of its parts have different levels of confidence:

| Change | Expected benefit and required evidence |
| --- | --- |
| One document owner, atomic semantic transactions, one browser lifecycle | High-confidence response to existing competing mutation paths and lifecycle ownership. Verify failure behavior and delete replaced paths. |
| Native/WASM source semantics | Rejected. Keep server compilation native-only and preserve the browser's existing JavaScript/Loro integration. Verify protocol compatibility with fixtures. |
| Commit-before-relay for ordinary typing | Preferred target: stronger visibility guarantee and fewer independently advancing server states. Validate available database capacity and choose an affordable, usable batching interval before switching production typing. |

Stage A must produce a representative commit-path benchmark, not only an inventory. Sections 10 and 27 define the evidence required. Browser compilation is deliberately outside the server build and deployment path.

The invariants and algorithms below describe the proposed destination, including commit-before-relay. They are not evidence that its costs have already been accepted. If a gate fails, revise this specification and its affected invariants before implementing an alternative; do not accumulate permanent optional architectures.

## 3. Product contract

### 3.1 Preserved behavior

1. Typing is immediately visible locally and does not require the network.
2. Prepared projects reopen offline with source, stable file identities, paths, settings, and locally available assets.
3. Creating, renaming, moving, and deleting source files works offline. Main-file changes work offline.
4. Routine concurrent text edits converge. A merge can still change meaning; convergence does not guarantee preservation of author intent.
5. Unsynchronized work survives a browser restart once its local persistence transaction succeeds.
6. Refused synchronization preserves recoverable local work and offers export.
7. A server acknowledgement means durable acceptance, not successful transmission.
8. Proposals, suggestions, and tracked edits use the same branch and decision model. Partial acceptance preserves operation authorship.
9. Comments keep their original identity and evidence. Uncertain attachment is explicit.
10. Historical states remain reconstructible for retained documents, including after long offline intervals. Compaction does not discard the Loro operation graph.
11. Plain source-and-asset export remains available. Users do not need a CRDT decoder to read an export.
12. Rendering remains outside the server document commit path.

### 3.2 Deliberate behavior changes

Other participants see a source edit only after its database commit. During a database outage, authors continue editing locally, but shared source does not advance. A source read never reveals an uncommitted candidate.

Relaying an uncommitted CRDT operation is not inherently a convergence error. Immediate relay with later durable acknowledgement is a valid alternative contract when surviving client copies, local persistence, and reconnect reupload are proven. A version vector alone is not a backup. This target chooses the stronger visibility rule to remove the server's long-lived live-versus-durable distinction; its cost must earn that simplification at the early gate. Under either contract, a durable comment or proposal decision must atomically include any source operations it depends on that are not already durable.

Source sharing can expose incomplete source as soon as it commits; there is no publication gate. A commenter may need to refresh an old render before submitting a passage comment. The typed comment draft must survive that refresh.

Deleting all source files produces a valid empty project. The editor offers file creation; the renderer reports that no main file exists. The server does not fabricate a text file as repair.

New or replacement asset bytes require successful online upload before a shared reference can commit. Offline asset creation is not added by this work.

### 3.3 Deferred product changes

Do not replace directory CRDT operations with online-only server commands. Although that would reduce some validation, it would weaken the existing offline contract and require a separate directory-command queue. Do not implement both models.

Do not remove proposal provenance, support for historical states, or comment evidence to obtain a smaller implementation. Any such change requires a separate product decision.

## 4. Required invariants

Implementation and tests must use these identifiers when stating which guarantee they establish.

| ID | Invariant |
| --- | --- |
| I1 | At most one admitted writer epoch can commit semantic document mutations in a deployment. |
| I2 | Within that writer, exactly one resident task owns mutable state for a document incarnation. |
| I3 | Authoritative source reads and broadcasts contain only database-committed operations. |
| I4 | A successful receipt identifies a committed result and survives connection loss and process restart. |
| I5 | Retrying a command with the same identity and payload cannot repeat its side effects. |
| I6 | Reusing a command identity with different content is rejected. |
| I7 | All state changed by a semantic command is committed atomically, including proposal or comment decisions related to source changes. |
| I8 | Authorization is checked against current durable authority at the commit boundary. |
| I9 | Browser project identity includes immutable server document identity; slug reuse cannot receive old local operations. |
| I10 | A stale connection or disposed session cannot import state, advance confirmation, or send new work. |
| I11 | Local persistence and remote durability are separate facts, each established by its own completed transaction. |
| I12 | Admission measures the final candidate, including server corrections, using the same definitions as durable storage. |
| I13 | File identity survives rename and move. Projection construction cannot silently discard colliding files. |
| I14 | Derived caches can be removed without losing source, original review evidence, or the ability to recover an anchor. |
| I15 | Compaction preserves every committed operation required by the retained-history and offline-merge contracts. |
| I16 | A non-editor receives no historical CRDT bytes, proposal bodies, deleted content, or privileged history through source projection or replay. |
| I17 | The system bounds queues, payloads, candidate memory, recovery work, and fan-out without discarding acknowledged work. |
| I18 | No client, route, worker, or administrative shortcut mutates the live document outside the owner. |

## 5. Target components

```text
Browser ProjectSession
  ├── document core: local optimistic source and branches
  ├── local durable store: source, pending work, receipts
  └── replaceable connection
                   │ commands / committed events
                   ▼
Transport and authorization adapters
                   │ authenticated command envelopes
                   ▼
DocumentRegistry → DocumentTask(document_id)
                    ├── committed DocumentCore
                    ├── bounded inbox and temporary candidate
                    ├── authorized subscribers
                    └── CommitStore → PostgreSQL transaction

Asset upload and archive workers → immutable blobs
                              → narrowly scoped database transactions
```

### 5.1 Document core

A small Rust library owns source semantics. It depends on Loro and the pure text/path algorithms it actually uses. It has no HTTP, SQL, filesystem, clock, account lookup, or async runtime dependency. IDs and actor identities are supplied explicitly where required.

It exposes domain operations, inspection, candidate preparation, projections, anchor algorithms, and branch review calculations. It does not expose arbitrary mutable root maps to ordinary application callers.

### 5.2 Document task

A Tokio task owns one document's committed core, a bounded inbox, subscriber state, and disposable caches. It has no public mutable `RoomState`. It executes semantic commands in order and returns typed results.

Use ordinary channels and concrete Rust types. A framework for interchangeable actors, supervisors, event buses, or dynamically registered command handlers is out of scope.

### 5.3 Registry

The registry maps immutable document IDs to a handle and load status. It provides one fallible admission API. There is no alternate `get()` that constructs an uncached room outside capacity accounting.

The registry owns task creation and eviction. Handles expose command submission and immutable read responses, never a mutex guard to document internals.

### 5.4 Commit store

The commit store executes a concrete document transaction. It accepts the expected writer epoch, expected document sequence, authenticated authority context, prepared source update, related row changes, and receipt data.

Keep this an application-specific transaction API. Do not build a generic SQL mutation language or a persisted executable command log.

### 5.5 Adapters

HTTP, WebSocket, CLI, MCP, and the local companion translate their input into the same domain command types. They may have different transport envelopes. They may not implement their own mutation ordering, source repair, proposal acceptance, or save logic.

## 6. Writer ownership and process failure

### 6.1 Deployment topology

The target supports one active writing backend per deployment. That backend owns all live document tasks. Multiple reverse proxies or static frontend hosts are harmless; multiple independent writing backends are not supported.

A passive backend must not serve authoritative live-room traffic or accept commands using cached state. Readiness indicates whether the backend holds writer ownership. Existing infrastructure can route to the active backend. This project does not add a proxy mesh or per-document distributed routing.

This deliberately trades horizontal write distribution for a much smaller consistency model. Measure a single backend before introducing document sharding in a separate design.

### 6.2 Ownership mechanism

Use a deployment-scoped PostgreSQL advisory lock held on a dedicated connection, plus a persistent monotonically increasing writer epoch. A process first acquires the advisory lock, then advances the epoch transactionally before becoming ready.

The advisory lock prevents normal concurrent activation. The epoch fences a process whose ownership connection was lost while other pooled connections remained usable.

Retain this protection unless a simpler mechanism demonstrates equivalent ownership-loss and takeover behavior. A startup advisory lock and document sequence compare-and-swap alone do not establish that a stale process cannot write: version ordering is distinct from writer authority. A single-process deployment configuration is not a failure test. Do not defer correctness until an overlapping writer is observed.

Every semantic document transaction must:

1. Acquire a transaction-scoped shared row lock on the deployment writer record and verify the expected epoch.
2. Acquire the target document row lock and verify status and expected sequence.
3. Check current authority and apply the mutation.
4. Commit before releasing these locks.

Epoch advancement takes a conflicting lock on the same writer row. Specify actual SQL lock modes in the implementation and prove that epoch changes cannot interleave after validation but before commit. Checking an epoch with an unlocked SELECT is insufficient.

Lock order is deployment writer record, document, authorization/quota rows in a documented stable order, then operation-specific rows. Shared quota operations must obey that order as well.

On ownership connection loss, stop accepting commands and close or drain connections. An ambiguous in-flight commit is resolved through durable receipts. A new owner recovers from PostgreSQL; it never adopts another process's mutable memory.

### 6.3 Background work

Compaction and archive materialization operate on immutable committed revisions. They do not append semantic source edits. Their activation transaction verifies its expected root/revision and updates only their own metadata.

Restore, source import, automated repairs, and administrative edits are semantic mutations and must use the document task. A live database maintenance script is not a supported alternative writer.

## 7. Document identity and revision model

Use the existing immutable document UUID as the incarnation identity. A URL slug is a lookup key. Browser caches migrate from creation-time identity to UUID only after verifying the mapping online; unidentified offline caches remain recoverable and unsent.

Use three distinct concepts, with no interchangeable aliases:

| Name | Meaning | Used for |
| --- | --- | --- |
| Commit sequence | Monotonic integer for every semantic commit on a document | Event ordering, receipts, coherent bootstrap |
| Source revision | Commit sequence of the latest commit that changed source | Rendering, anchors, checkpoints, restore preconditions |
| Loro frontier/vector | Exact operation-graph position or causal knowledge | CRDT reconstruction, branch bases, delta synchronization |

A comment-only commit advances commit sequence but leaves source revision unchanged. Presence advances neither. Compaction and cache materialization advance neither. No-op synchronization can return the current revision without creating a source commit.

Integers in JSON are decimal strings so JavaScript cannot truncate a database sequence. Binary frontier/vector encodings are opaque and explicitly versioned.

A retained source revision resolves to its immutable Loro frontier. It must continue resolving after update-row compaction. A source revision identifier is not an access credential.

Avoid additional logical generation counters when one of these concepts answers the question. A browser lifecycle generation and deployment writer epoch are separate ownership tokens, not document revisions.

## 8. Command contract and retry semantics

### 8.1 Envelope

An illustrative domain envelope is:

```rust
struct DocumentCommand {
    document_id: DocumentId,
    request_id: RequestId,
    expected_source: Option<SourceRevision>,
    payload: CommandPayload,
}
```

Authentication and authority context are supplied by the server adapter, not trusted from payload fields. Common correlation fields belong in the envelope rather than repeated across every enum variant.

Representative payload variants are `MergeSource`, `CreateComment`, `Reply`, `SetCommentResolved`, `OpenProposal`, `UpdateProposal`, `DecideProposal`, `CreateCheckpoint`, `Restore`, and `AttachAsset`. Directory operations are local core operations whose CRDT updates arrive through `MergeSource`.

Use tagged deserialization directly into validated transport types. Remove the giant optional-field `Message` followed by a second parallel conversion model once old clients are retired.

### 8.2 Preconditions

Ordinary CRDT source merges do not require an exact current source revision; concurrency is expected. They require the correct document incarnation and causal dependencies.

Commands whose meaning depends on observed state carry a precondition: proposal base/tip, comment version, target source revision, or expected current source for restore. A stale command returns a typed conflict with enough permitted information to refresh. It is never silently reinterpreted against a new target.

### 8.3 Receipts

Persist receipts for commands with non-idempotent semantic effects. Key them by document, authenticated principal context, and request ID. Store a digest of the canonical command and its stable result: status, committed sequence, source revision, and created object IDs.

A repeated matching request returns its prior result. A repeated ID with a different digest is rejected. Reauthorization still applies before returning protected result data; receipt lookup does not restore revoked access.

For pure CRDT synchronization, use Loro's idempotent operation identity and durable vector coverage rather than retaining a receipt row per keystroke forever. A retry that contributes no new operations returns the current durable coverage without an extra source log entry.

Clients must persist semantic request identities before dispatch. There is no automatic offline outbox for future restore, accept/reject, or sharing actions. Unconfirmed submissions may be retried with their original ID; a brand-new action requires a current user decision.

Initially retain semantic receipts for the document's lifetime. Their scope excludes ordinary typing. Any future receipt expiration design must define safe retry behavior before deleting them.

### 8.4 Ambiguous database outcome

A lost database response is neither acceptance nor rejection. The task enters recovery for that operation, checks the receipt or durable source coverage using a new connection, and resumes only after determining the committed state.

Do not install a candidate on an assumed commit. Do not execute a non-idempotent command again under a new request ID. If recovery is unavailable, disconnect clients with a retryable status and preserve their work locally.

## 9. The single commit algorithm

The owner executes the following steps for a semantic command or bounded source batch:

1. Verify document identity, command shape, queue limits, and known authority before expensive work.
2. Inspect the committed state and check the command's preconditions.
3. Prepare exactly one candidate, or candidate source transformation plus related row changes.
4. Validate the final candidate's schema, causal completeness, paths, file count, source bytes, assets, and encoded history.
5. Produce the exact delta from the committed state and the resulting frontier. Record any server corrections in that same delta.
6. Execute one commit-store transaction with writer fencing, current authorization, quota admission, sequence compare-and-swap, source delta, related semantic rows, revision metadata, and receipt.
7. After confirmed commit, install the prepared candidate as the committed core and replace affected committed metadata.
8. Invalidate derived caches by revision, then emit an ordered committed event and successful response.

If preparation or transaction validation fails, discard the candidate. No inverse edit or memory rollback is needed because the authoritative core was not changed.

If installation fails after commit, stop the task and recover from storage. Do not continue serving a state whose sequence differs from the durable one.

During a commit, the task does not execute another mutation. Read requests may receive the previous immutable committed projection or wait for the task. Both are coherent. They must never inspect candidate memory. A read that promises read-after-write consistency must wait until its requested commit sequence is installed.

Network sends never block the mutation transaction. Slow consumers have bounded queues and reconnect to recover committed state.

### 9.1 Batching

Batch only ordinary source updates. Explicit commands form ordering barriers and are not silently delayed behind a long typing window.

Initial benchmark candidate: a maximum 500 ms source batching window, with earlier closure at a bounded byte limit or command barrier. Compare 500 ms, 1 second, and 2 seconds before selecting the production setting. These are measurement candidates, not established latency or cost guarantees. Local edits remain instantaneous; longer windows delay collaborator visibility under this target.

Preserve per-origin order. Admit each contribution against the evolving candidate so one invalid update does not reject unrelated valid contributors. Maintain per-contribution validation checkpoints only within the bounded batch; do not recreate a second long-lived speculative room.

If a later update depends on a refused update, return an explicit dependency/refusal result to that origin. Do not accept pending dependencies for unlimited future application.

At transaction time, revalidate every contributor's authority. If any contributor has lost access, discard and rebuild the batch without its contributions and their dependents, then revalidate limits. Bound rebuild attempts; under churn, retry contributors individually. No source from a revoked contributor may be committed merely because a batch was prepared earlier.

### 9.2 Cost consequences

The existing 30-second save cadence can approach two source transactions per active minute. A continuously active document with 500 ms batches can approach 120. This is roughly a 60-fold difference in source transaction frequency before workload effects, even if each write is small.

Measure transaction count, WAL bytes, database CPU, update-row growth, compaction writes, and remote visibility latency. Removing archives and attachment writes may offset part of the cost, but do not assume it offsets all of it.

Use the current post-publication-removal, frontier-comment implementation as the baseline. Savings already delivered by those changes are not offsets attributable to this project. More commits do not imply proportionally more source bytes or a proportionally larger managed-database bill: measure transaction overhead and the provider's actual compute, storage, I/O, backup, and transfer charges. Report application-server candidate preparation CPU and memory separately from database cost. Rendering remains client-side and contributes no server rendering cost.

For provisioned managed PostgreSQL, extra commits may fit within already-paid capacity and leave the compute bill unchanged. The preferred commit-before-relay target should not be rejected merely because its transaction count is higher. Conversely, a small stored database does not prove low CPU or durable-write I/O demand. Record actual database/history size and workload before making claims about deployment cost; neither production size nor spare capacity is established by this specification.

Do not hide a second visibility-before-durability mode behind a configuration flag. If the target is too expensive, lengthen the same batching window and evaluate the collaboration experience, or revise the design explicitly. Keep one commit model.

## 10. Native authoritative document core

### 10.1 Package boundary

Extract a focused native workspace library, provisionally `librepaper-document-core`. It is compiled only for the server and must not become a browser or deployment-time WASM build step. The name can change; the dependency boundary cannot.

Retain the existing Loro encoding and four-map schema initially: `files`, `paths`, `assets`, and `meta`. Package extraction must not regenerate operation identities or rewrite all documents.

One browser session retains one underlying `loro-crdt` document. Do not introduce a second document, a Rust/WASM binding, or full-project snapshot shuttling. Preserve `SPEC-loro.md` section 5.2's local diff computation contract. Prove UTF-16 conversion, stable identities, and wire compatibility with serialized native/browser fixtures; the implementations remain platform-specific.

### 10.2 API categories

| Category | Examples | Responsibility |
| --- | --- | --- |
| Source editing | Insert/delete text by file ID | Preserve Loro operation identity and UTF-16 boundaries |
| Directory operations | Create, relocate, remove, set main | Validate intent and emit one local transaction |
| Import preparation | Apply incoming update to candidate | Bound decoding, validate roots/types, resolve permitted merge collisions |
| Projection | Current tree, format, assets | Produce one canonical, collision-free view |
| Review | Fork, diff, hunk identity, selective acceptance | Authoritative server hunk rules and provenance |
| Anchors | Capture and resolve original evidence | Source identity independent of rendered layout |
| Measurement | Source bytes, files, actual encoded history | One definition per limit |

Raw Loro operations are confined to the native core on the server and the existing editor integration in the browser. The server treats incoming bytes as hostile even when generated by the official browser.

### 10.3 Schema and validation

Define allowed roots, container kinds, metadata keys/types, path rules, and digest encoding explicitly. Reject unsupported roots and container shapes. Do not maintain a recursive cost estimator for arbitrary structures the product never uses merely to allow them into the document.

Validation covers the operation history and pending imports sufficiently to prevent an unsupported root or oversized payload from being hidden behind deleted/currently unreachable state. Visible-text measurement alone is not a security boundary. Use encoded-history and import resource limits as well.

Unknown future schema versions trigger an upgrade-required response. They are not repaired into the current schema. User-visible render settings must have explicit versioned fields; adding a setting requires updating that schema.

All boundary offsets use UTF-16. Internal library diff bases must be converted within the core or kept internal. Cross-platform fixtures must include astral characters before and inside reviewed ranges. Sharing source code does not eliminate platform-dependent Loro behavior.

### 10.4 Bounded normalization

Replace broad best-effort repair with a narrow deterministic normalization step for states that valid concurrent operations can produce. Malformed container types and invalid encodings are refused. Valid path collisions are resolved without losing files.

Build a complete ID-to-final-path assignment from the candidate before writing any correction. Resolve conflicts in a documented stable order, considering case folding, Unicode normalization, existing suffixed names, length limits, and text/asset collisions. Write paths by ID, never by reverse lookup of an ambiguous old path. A second normalization pass must make no changes.

Inputs whose names cannot be normalized safely receive a refusal with recovery information. Never delete asset references merely to conceal a collision. Retain stable text file identities; asset moves remain explicit map operations until a separately justified asset-identity migration exists.

Main-file selection may be absent in an empty project. If a concurrent deletion removes the selected main file, select a remaining renderable file deterministically or leave main unset when none exists. Preserve an existing valid selection. Do not create content to repair an empty project.

Concurrent delete/edit may leave an edit only in retained history. Before reconciliation hides locally edited content, the browser must retain an accessible recovery copy. The UI reports the removed file and offers recovery/export; it must not advertise semantic losslessness merely because the operations remain in Loro.

## 11. Admission, quotas, and bounded resources

Keep distinct measurements distinct:

| Measurement | Definition | Enforcement |
| --- | --- | --- |
| Current source bytes | UTF-8 bytes of canonical current text plus explicitly defined metadata charges | Final candidate |
| File count | Current visible text files and asset entries under the documented policy | Final candidate |
| Encoded history bytes | Actual export of complete retained Loro history in the chosen canonical encoding | Final candidate |
| Durable physical bytes | Stored update rows, bases, assets, archives, and retained semantic records | Transactional storage accounting |
| Resident memory | Conservative measured/estimated live core, candidate, queues, projections, and review data | Runtime admission |

An accumulated delta-byte estimate may trigger exact measurement. It must not fence a document against an encoded-history limit by itself. Compression ratios and serialized byte lengths must not be treated as resident-memory measurements.

Initial implementation may perform one complete encoded-history measurement per candidate batch. Remove repeated imports and exports first. Optimize with measured, conservative fast paths only after profiling, and retain exact final checks whenever the bound cannot decide.

Decode limits apply before materializing unbounded data. Reject truncated, malformed, causally incomplete, over-compressed, oversized, and pathological input without modifying committed state. Rate-limit per principal and document before expensive preparation. Bound synchronous CPU work; run heavy preparation on a bounded worker pool if it would stall the async runtime. Preserve exclusive command ownership while awaiting that work.

Asset quotas include physical bytes and document references according to existing policy. A history budget refusal preserves the previous committed state and returns a usable recovery path. It does not permanently mark the room corrupt.

## 12. Durable data model

Prefer adapting existing tables to creating parallel models. The following logical records are required; names are illustrative.

### 12.1 Document head

The document row contains immutable identity, lifecycle status, current commit sequence, current source revision, and current schema version. Writer ownership belongs to the deployment record. Source format and main path may be indexed projections but are updated in the same source transaction and are never independent authorities.

### 12.2 Source updates and revision index

Each source-changing commit records the exact Loro update and resulting frontier. A compact revision index retains the source revision, frontier, actor/time metadata, and schema/encoding version after bulk update rows have been compacted.

Do not retain an unbounded duplicate copy of update bytes in an event table. The update log is the durable source byte stream; the revision index is metadata.

### 12.3 Semantic tables

Comments, replies, proposal branches, decisions, checkpoint labels, and receipts remain ordinary relational records. Commands touching more than one table use a shared SQL transaction. Refactor repository methods that always create and commit their own transaction so they can participate in the document commit.

This is not full event sourcing. Do not rebuild comment or sharing state by replaying a newly invented general command log. Source recovery uses Loro operations; relational state uses its existing tables.

### 12.4 Committed event delivery

The in-memory event stream reports committed results. There is no durable delivery guarantee for a particular socket. After a missed event, clients use coherent bootstrap or source vector synchronization and refetch allowed relational projections.

Retain a bounded in-memory replay window only if useful. Do not add a persistent outbox solely to promise delivery of replaceable UI notifications. A future integration requiring durable external delivery must specify its own transactionally written outbox and is outside this work.

## 13. Checkpoints, archives, and restore

### 13.1 Checkpoints

A checkpoint is a label/event referencing a committed source revision. Creating it is a small semantic transaction. It does not force another source update, re-encode the source merely to establish durability, or write a complete source archive synchronously.

Checkpoint rows include source revision, actor, time, and label/reason. Repeated requests with the same request ID return the same checkpoint. Explicitly different labels on the same revision may coexist; deduplication of byte storage must not erase meaningful user labels.

### 13.2 Archives

A plain-source archive is generated from one immutable source revision. Its worker verifies the canonical tree digest, asset references, and encoding version before attaching the immutable blob reference.

Explicit checkpoints schedule archive materialization so an independent source copy eventually exists. Export requests can generate or wait for that same artifact. Comments and routine autosave do not schedule archives.

Use at most one stored archive per equivalent canonical source tree and archive encoding, with labels referring to it. Scheduling and retries must not create a new source revision. An archive pending or failed state is separate from source durability.

This changes the timing of format-independent copies: live committed work is immediately recoverable through Loro, while an independent archive is available after materialization. The UI and backups must not imply every keystroke already has a plain-file archive.

### 13.3 Restore

Restore prepares a transformation from the current committed source to the chosen historical revision. It preserves file IDs where appropriate and produces new operations; it never rewinds the authoritative graph in place.

The command carries the source revision the user confirmed. If that revision is no longer current at execution, return a conflict and preserve the prepared result for refresh only if bounded. Do not silently overwrite edits that arrived after confirmation.

Commit the restoring delta, any before/after labels, and receipt in one transaction. Installation follows commit. There is no preliminary live mutation followed by a compensating rollback routine.

## 14. Comments and derived attachment

### 14.1 Original evidence

Persist immutable document ID, source revision, file ID, selected source range, source quotation/context, versioned original cursor evidence, and permitted rendered presentation evidence. Validate that they agree with the referenced committed source before accepting the comment.

The browser render coordinator labels every render with the source revision or local working revision it represents. A render of uncommitted local work cannot be used as if it were a committed source revision. Flush and confirm its source, or preserve the draft and ask the user to retry against a current render.

For viewers/commenters, use an opaque current-render token or current source revision under current read authorization. If the source has moved, initially return a stale-selection response and refresh. Do not give non-editors historical source access merely to locate an old selection. Existing accepted anchors are resolved server-side using their already authorized records.

### 14.2 Resolution

Resolve an anchor as a pure function of original evidence and the target source revision, with access to retained history. A cache may accelerate it but must not be the only surviving source of cursor identity.

Return explicit attached/modified/deleted/ambiguous/unresolved outcomes. Do not use the first textual match to conceal ambiguity. Do not move original evidence to follow a new match.

### 14.3 Cache behavior

Cache by annotation identity, anchor schema version, and target source revision. Invalidate by revision. Resolve visible annotations on demand and coalesce refreshes after source commits. Start with bounded in-memory caches; no durable current-position table is required for correctness.

Deletion of all caches followed by restart must reproduce equivalent resolution. A warm cache and a cold resolver must agree on identity and status, even if their diagnostic detail differs.

Before removing current persisted live cursor fields, migrate or reconstruct sufficient immutable evidence at the original revision. Rows that cannot be reconstructed retain their legacy evidence and explicit unresolved status; they are not silently reassigned or dropped. Compatibility reads are removed only after the migration audit proves coverage or records explicit exceptions.

## 15. Proposals, tracked edits, and agents

One proposal consists of a base source frontier, branch operations, a versioned review model, and server-owned decision records. Browser tracking, comment suggestions, and agent changes differ in how they create the branch, not in how they are accepted.

Store branch identity and base/tip preconditions explicitly. A decision refers to the reviewed base and tip and stable hunk identity from the authoritative native core. The browser submits reviewed identities; it does not independently determine the durable hunk set.

Partial acceptance prepares a candidate using provenance-preserving branch import and rejected-hunk reversal. The intermediate imported-but-not-reversed state is confined to candidate memory. Commit the resulting source delta, hunk decisions, proposal status, related comment resolution, and receipt atomically.

Author identity must be established from authenticated proposal creation and accepted operation attribution rules. Client-provided peer IDs or display names are not proof of account authorship. Preserve existing provenance guarantees and test attempted attribution forgery before cutover.

Agent direct edits, where authorized, use the same candidate and commit path. Agent tools do not hold a live room lock while running a model, accessing a local filesystem, or compiling. They work against an immutable revision and submit a proposal or a preconditioned patch command.

Previewing proposals uses a browser scratch branch. It cannot advance server confirmation or generate an archive/checkpoint on its own.

## 16. Browser project lifetime

### 16.1 Ownership

One `ProjectSession` owns the local core, local persistence adapter, durable confirmation knowledge, pending semantic request identities, transport lifetime, and recovery state. `Reader.svelte` observes it and invokes operations. It does not orchestrate reconnects or own a second source document.

Rendering and presence are child resources with explicit disposal. Presence remains ephemeral and is not put into the source commit queue or database log.

### 16.2 State model

Represent connection state with a discriminated state, such as disconnected, connecting, handshaking, ready, blocked, or disposed. Keep local persistence state and remote durable coverage separately. Do not infer either from connection state.

A connection gets a monotonically increasing in-memory generation and cancellation signal. Increment the generation before starting a replacement connection, disconnect recovery, or disposal. Every asynchronous continuation checks it after each await and before importing, sending, or reporting success.

`dispose()` is idempotent and owns all subscriptions, transport callbacks, heartbeat/retry timers, presence expiry timers, pending fetches, and core handles. No callback after disposal changes UI or storage ownership.

### 16.3 Confirmation

The client stores its local Loro state and durable server vector coverage. A local operation is remotely confirmed only when the acknowledged committed coverage includes it. A socket sequence number alone is not evidence after a restart or tab change.

The session need not retain duplicate byte arrays for every unacknowledged keystroke. It derives missing updates from the local CRDT and confirmed server coverage. Semantic request receipts remain separate because source convergence cannot prove a comment was posted or a proposal was decided.

An acknowledgement and the source state it describes can arrive in either order. Persist confirmation with enough project identity and causal metadata to interpret it safely. If local receipt persistence fails, display conservative confirmation after restart and resynchronize; never discard work on the basis of an unpersisted assumption.

## 17. Local persistence and multiple tabs

IndexedDB storage is keyed by origin, immutable document ID, and appropriate account/access context. Changing accounts cannot silently expose another account's prepared project. Previously authorized data cannot be remotely erased from a disconnected device, but the application's cache selection must respect local account boundaries.

Store source history and local confirmation metadata using recoverable transactions. Do not keep the pending-work truth only in memory. Browser storage eviction remains outside the durability guarantee and must retain the existing export guidance.

Each tab has a distinct Loro peer identity and pending-work attribution. Use append-only local update records with unique operation/batch identity and transactional compaction, or an equivalently proven transactional merge. Two tabs must not overwrite a common full snapshot independently. BroadcastChannel may notify peers but is not a correctness dependency.

Compacting local updates must merge all included records in the same serialized storage operation and atomically switch the base. Concurrent appended updates remain reachable. Persisted confirmation from one tab is reusable only as actual durable vector coverage; it must not mark another tab's unrelated work as saved.

Reopening offline hydrates local state before network synchronization. Online metadata checks are not a prerequisite to local recovery. They are a prerequisite to sending old work to the server.

Source refusal, stale schema, permission revocation, deletion, or incarnation mismatch moves the session to a recoverable blocked state. Stop automatic refusal/reconnect loops. Keep export and local inspection available without fetching newly unauthorized remote content.

## 18. Synchronization and transport

### 18.1 One domain protocol

Use one versioned command/result model for durable operations, regardless of adapter. WebSocket can carry browser source updates and commands; HTTP/CLI/MCP invoke the same dispatcher. Do not implement different save behavior in a fallback comments endpoint.

A browser may explicitly retry the same semantic request over HTTP if needed. The original request ID is preserved. Automatic transport fallback cannot turn an unconfirmed socket submission into a new comment.

### 18.2 Coherent handshake

1. Hydrate the browser's local project.
2. Resolve and authorize the immutable remote document identity and protocol/core schema versions.
3. Register the connection with the document owner and capture a committed baseline sequence and source vector.
4. Return authorized relational projections and the required committed source difference, or a reference to an immutable baseline transfer.
5. Deliver committed events after that baseline in order, with bounded buffering.
6. Reconcile local source against confirmed remote coverage, upload missing operations, and wait for committed acknowledgement.

Registration and baseline capture must have a single ordering point inside the document task. A commit cannot disappear between fetching a snapshot and subscribing. If buffers overflow, invalidate that handshake and restart from a newer baseline.

An initial transfer reference identifies exact immutable bytes and baseline sequence. It must not be a URL that happens to return whichever room state exists when fetched. Keep large transfers in bounded memory/streaming infrastructure; a routine reconnect does not create a durable source archive. Expiration or eviction restarts the handshake.

Every transfer request rechecks authority. Reference signatures and digests establish integrity/identity, not permission. Do not encode editor history into a publicly fetchable reference.

### 18.3 Payload framing

Prefer binary frames for source bytes with a small versioned envelope. Retain bounded chunk assembly only where transport limits require it. Define maximum total bytes, chunk count, deadline, and permitted ordering. Reject incomplete transfers on disconnect and recover by vector synchronization.

Do not maintain separate full-state resend, delta resend, and compatibility admission pipelines. Both full-state and delta inputs enter the same candidate validator. Compatibility encoding adapters are temporary edge code.

### 18.4 Events and missed delivery

Committed events carry document ID and commit sequence. Editor source events include the committed update and source revision. Non-editor events contain only allowed change notifications and review data.

On a sequence gap, resynchronize source by vector and refetch permitted relational projections. If the sequence range contains filtered events, use explicit stream baselines/watermarks rather than assuming every global commit must have a visible payload for every role.

On reconnect, a receipt can prove a prior command committed even if its broadcast was missed. Bootstrap correctness must not depend on an in-memory replay buffer surviving restart.

### 18.5 Presence

Presence belongs to connection membership and replaceable ephemeral state. It uses bounded payloads and throttling, can be lost, and cannot authorize source changes or establish authorship. Heartbeats renew active membership; inactivity/disconnect expires it. Late joiners receive current authorized presence or a bounded reannouncement, rather than relying on another user to type.

All presence timers and subscriptions are disposed with the connection/session that owns them. No durable receipts, source revisions, or database writes are created for cursors or colors.

## 19. Current-source reading and rendering

Every authoritative projection is built from one committed source revision: text, main file, format/settings, and asset references agree. The projection is an allowlisted structure; it is not a serialized room with privileged fields removed afterward.

Readers and commenters receive current inputs and current asset access under existing policy. Editors receive CRDT synchronization only after editor authorization. History routes, branch bytes, and old assets remain separately protected.

Conditional GETs check authorization before returning 304. Projection digests are cache validators, not credentials. A notification causes a coalesced refetch and render; it never writes a checkpoint or a server render artifact.

An editor's local preview may include unconfirmed work. Label its internal render identity accordingly so source selection and diagnostics cannot be mistaken for a committed revision. User-facing status should remain plain: locally saved, syncing, synced, or blocked.

## 20. Assets and external computation

Asset upload writes immutable bytes and registers a staged, authorized reference with bounded lifetime and transactional quota reservation. The bytes are verified before a command may attach them to a committed document.

The document commit checks that every referenced digest exists, is authorized for this document, and satisfies quotas. Digest knowledge alone is not permission to reference another document's private object. Current asset access checks current committed reachability.

Upload failure or an uncommitted attachment does not alter the live directory. Orphan cleanup reclaims unreferenced staged bytes after a grace period. Referenced assets are pinned by live state, retained historical states, proposals, or in-progress materialization as required.

Quarto execution, local compilation, Zotero access, and assistant processes operate outside document ownership. They return artifacts or proposed source changes with identity/revision preconditions. The document task must never wait on arbitrary user code or a model response.

## 21. Compaction, retention, and backups

Retain the existing compressed-base plus update-log storage model. A compactor reads a coherent committed cut, produces a complete history base through that cut, verifies it, and atomically activates it with the expected compaction root. Newer updates remain outside the base and reachable.

Retain source-revision-to-frontier metadata after update-byte reclamation. Recovery must distinguish already compacted rows from missing/corrupt history. Remove active-writer reconciliation, not recovery gap detection or integrity checks.

No-op reads, reconnects, presence, cache refreshes, and compaction create no semantic source revisions.

The retained-history contract means deleting a label is not permission to discard the operation graph or assets needed by other retained states. Before pruning any source or blob, enumerate its reachability roots. Whole-document deletion follows the existing lifecycle and retention policy; it is a separate authorized operation.

Backups capture PostgreSQL and referenced immutable objects coherently. They include writer/schema metadata, revision indices, semantic receipts, proposals, anchor evidence, and any pending archive references. Restore starts a new valid writer epoch and reconstructs rooms from committed data only. Restoring a backup does not allow old browser receipts to prove the restored server contains operations it no longer has; the restored deployment needs a recovery/incarnation marker that forces durable coverage revalidation while preserving local work.

## 22. Room admission and eviction

One registry API either returns a valid owner handle or a typed capacity/recovery error. Concurrent loads of the same document share one loading slot and one recovery attempt. Failed or cancelled loads release reservations.

Bound resident document count and memory, including candidate overhead and buffered transfers. Reserve enough headroom before preparing a large candidate. Slow recovery for one document must not hold a global registry mutex across storage I/O.

A task is evictable when it has no active subscribers, no accepted commands awaiting completion, no in-flight commit, and no transfer that pins its state. There is no dirty-document exception because installed state is already durable.

Avoid using incidental `Arc::strong_count` as the domain definition of activity. The registry/task protocol explicitly accounts for active requests and reservations. Reference ownership still prevents use-after-free, but admission decisions have named states.

Use one bounded idle eviction mechanism. There is no once-per-second sweep that queries storage for every idle room, reconciles remote writes, flushes source, and separately flushes attachments. Storage jobs retain their own scheduler.

## 23. Error model and user recovery

Use a small stable set of typed failures with adapter-specific presentation:

| Failure | Server behavior | Browser behavior |
| --- | --- | --- |
| Invalid schema/payload | Refuse before commit | Preserve work; stop automatic replay; offer recovery |
| Causal dependency missing | Refuse incomplete merge with permitted sync guidance | Reconcile vector and retry boundedly |
| Source/history/asset limit | Leave committed state unchanged | Keep local work; offer reduction/export |
| Authority revoked | Refuse transaction and terminate privileged stream | Stop sends/fetches; retain permitted local recovery |
| Stale semantic precondition | Return conflict, no mutation | Refresh target while preserving draft |
| Capacity/backpressure | Retryable refusal before acceptance | Retain local changes; back off |
| Storage unavailable | No new authoritative state | Continue local editing; report unsynced state |
| Commit outcome unknown | Recover before executing further mutation | Keep original request identity; wait/retry |
| Corrupt durable state | Refuse recovery; no empty fallback | Show unavailable state; allow local export |
| Ownership lost | Fence writer and invalidate connections | Reconnect and verify durable coverage |
| Unsupported client version | No import or mutation | Upgrade without deleting local state |

Do not conflate quota refusal, corruption, revoked access, and temporary storage failure into one permanent `read_only` flag. The owner has an explicit lifecycle/failure reason, and retryability is defined by that reason.

## 24. Migration strategy

### 24.0 Implementation handoff and execution order

When asked to implement this specification, carry the work through the completion criteria in section 28. This is an implementation assignment, not a request for another architecture proposal. Begin with the three bounded work packages below, use their evidence to guide the remaining stages, and continue through integration, migration tests, and deletion of replaced paths. Passing a prototype or introducing a new abstraction alone does not complete the project.

Preserve unrelated working-tree changes. Establish the starting revision and local modifications before editing, read applicable repository instructions, and re-resolve every cited call site. Keep a checked-in ledger at `docs/specs/document-architecture-implementation.md` containing the initial inventory, work-package status, decisions, benchmark commands/results, failure-test evidence, migration boundaries, and deleted responsibilities. Record assumptions and unresolved items explicitly. Do not claim production deployment, production database measurements, or acceptance of a deployment budget from local tests. Implement and validate locally; this specification is not authorization to deploy or perform destructive production migrations.

The initial investigation on 2026-09-19 established the following starting points. They are leads to verify against the implementation checkout, not substitutes for reproducible evidence:

| Area | Evidence and limitation |
| --- | --- |
| Browser | The existing npm Loro/CodeMirror integration passed 11 editor browser scenarios and 8 focused tests. The rejected Rust/WASM binding experiment is not part of the target; serialized interoperability remains the required boundary evidence. |
| Source storage | Persistence exports a delta relative to `durable_vector`; PostgreSQL appends that delta and frontier. Consolidated synthetic history measurements were small, but do not measure PostgreSQL transaction/WAL overhead or production database size. |
| Database benchmark | The existing catalog benchmark uses placeholder source updates and retains bundle-oriented workloads. A disposable-database run failed on a storage-usage constraint before yielding a usable report. It is not capacity evidence for this design. Some Makefile measurement targets also name tests absent from the investigated tree. |
| Semantic transactions | `storage/postgres/proposals.rs::resolve_with_update` already couples source append and proposal resolution. `room/proposals.rs::decide_hunk` persists hunk decisions separately; agent source edits, checkpoints, and comment updates also span separate operations. Extend the useful existing transaction pattern rather than assuming no atomicity exists. |
| Ownership | `Room.state` remains public, mutation ordering is spread across gates and adapters, and the sweep reconciles remote updates. A startup writer-lock comment was found without a corresponding deployment writer-lock implementation; the PostgreSQL migration lock is not deployment fencing. |
| Browser lifetime | Responsibilities span `project-session.js`, `reader/collaboration.js`, `room.js`, `collab.js`, and `Reader.svelte`. The existing project session is the migration starting point, not a reason to introduce another owner. |

#### Work package 1: atomic proposal acceptance as the first implementation slice

Start here because it delivers a correctness improvement independently of the WASM binding decision and typing cadence. Inspect `room/proposals.rs`, `room/agent.rs`, `room/comments.rs`, and `storage/postgres/{proposals,annotations,collaboration}.rs` together with their callers.

1. Introduce a concrete caller-owned transaction boundary that can append the exact source delta and update related hunk decisions, proposal status, comment resolution, revision metadata, and semantic receipt together. Adapt existing repository operations to use that transaction; do not nest independently committed methods.
2. Prepare the candidate without changing the installed document. Include any previously unsaved source operations required by the result so a durable semantic record never names source absent from storage. Preserve partial-decision behavior: commit all effects of the particular command atomically, without inventing source changes for a decision that changes no source.
3. Check current authority, reviewed base/tip, expected durable sequence, quotas, and command identity in the transaction. Use the section 8 receipt contract, including payload mismatch and reauthorization. Add writer-epoch enforcement with the deployment ownership work before declaring this path complete for the final architecture.
4. Install and notify only after confirmed commit. Resolve uncertain commits from durable state and the original receipt; do not rerun with a new request ID. Keep existing external interfaces usable while migrating the internals, without leaving an alternate final write path.
5. Prove rejection before commit leaves source, decisions, proposal status, comment resolution, and notifications unchanged; prove success updates the affected records together. Test lost response after commit, process restart and retry, duplicate identity with changed payload, stale tip, partial acceptance, concurrent source edits, and permission revocation.

Exit evidence: integration tests with real Loro and disposable PostgreSQL for I3–I8 as applicable, a documented remaining ownership/fencing dependency, and removal of the migrated command's separate-write/compensating-update path. This slice is a seed for Stage C, not completion of every semantic command.

#### Work package 2: browser/server protocol compatibility

The custom native/WASM binding is rejected. Preserve the existing npm Loro/CodeMirror integration and prove compatibility with the native authoritative validator at the serialized protocol boundary.

1. Inventory the actual Loro APIs used by the vendored CodeMirror integration, proposals, MergeEditor, and offline persistence. Identify application dependencies in `document/session` that must become explicit inputs or pure helpers, including ID generation, path rules, configuration, and text-edit types.
2. Keep browser file creation/lookup, text edits, import/export, subscriptions, editor, and undo on the existing single `loro-crdt` document. The server build must not compile browser code or a WASM binding.
3. In a real browser, edit the same text through a core operation and CodeMirror, import a remote update, switch files, undo/redo, persist, and reopen offline. Demonstrate that each path observes the same underlying state. Exercise proposal preview/partial acceptance and MergeEditor sufficiently to expose API or coordinate incompatibilities before full migration.
4. Compare native and browser operation identity, source projection, Unicode coordinates, cursor resolution, and stable hunk identity with serialized shared fixtures. Include astral characters and combining marks. Document protocol and Loro version compatibility.
5. Record bundle size, startup/edit latency, memory, required consumer migrations, maintenance responsibilities, and effort assumptions. Retain reusable prototype tests in the eventual production integration; remove throwaway scaffolding after migration.

Exit evidence: browser interoperability fixtures using its existing single document, corresponding native validation results, and proof that server builds do not compile the browser or a Rust/WASM binding.

#### Work package 3: a realistic durable commit benchmark

Build a dedicated repeatable benchmark using real Loro updates and an explicitly created disposable PostgreSQL database. Reuse sound fixture/statistics helpers, but do not rely on the old catalog benchmark's placeholder bytes or disable accounting constraints to obtain favorable results. Fix benchmark defects separately when necessary. Never point destructive fixtures at an existing user database.

1. Establish a baseline from the current implementation and a candidate/commit-path measurement using the same fixtures. Begin with 100 KB, 1 MB, and 4 MB source, within configured supported limits; add small visible source with long edit/delete history. Include one author and three independently identified concurrent authors. Document corpus, seeds, edit rate, and history generation.
2. Compare the current persistence cadence with 500 ms, 1 second, and 2 second source batches. Exercise 1, 10, and 100 continuously active documents where supported; report capacity failures rather than silently lowering the load. Use an explicit steady-state duration, targeting ten minutes per representative configuration after a short smoke run. Avoid claiming every account continuously edits.
3. Measure candidate preparation, final history measurement, commit latency, and end-to-end collaborator visibility separately. Record p50/p95/p99, throughput, failures, peak application memory, database CPU where available, WAL bytes, table/index size, update rows, compaction activity, and idle query rate. Record unavailable metrics rather than estimating them as observations.
4. Use normal durable database settings and verify the benchmark environment's configuration. Record PostgreSQL version, hardware, connection pool, fsync/synchronous-commit settings, application blob durability, and network latency. Keep CRDT payload bytes, compressed consolidated history, physical database bytes, and WAL traffic distinct. Include compaction's steady-state cost and historical reconstruction correctness.
5. Record raw results and reproducible commands in the ledger or linked artifacts. Apply section 27's declared performance/budget thresholds and select the batching interval. A local benchmark can establish local capacity, not managed-provider latency or a production monthly bill; report a parameterized cost model until provider, activity, and deployment measurements are available.

Exit evidence: a working benchmark with realistic deltas, retained-history recovery verification, an explicit capacity/latency assessment, and a batching recommendation. The old benchmark's failure and small compressed histories are not substitutes for these results.

#### Continue through the redesign

The three packages above are the opening work, not the final deliverable. They can proceed independently where resources allow; no parallel-agent execution is required. Use this dependency order for the remaining implementation:

| Step | Work and prerequisite | Required exit evidence |
| --- | --- | --- |
| 1 | Extend package 1's transaction boundary to the remaining semantic commands (Stage C). | Source-dependent records and retries are atomic; no independent inner commits remain in migrated paths. |
| 2 | Introduce the private document owner and migrate every mutation adapter; implement deployment lock/epoch fencing (Stage D). | Socket, HTTP, CLI/MCP, agents, restore, checkpoints, asset-reference changes, and administrative operations cannot bypass ownership. Ownership-loss/takeover tests pass. |
| 3 | Implement the compatible client protocol and browser lifecycle needed for the typing cutover (Stage E). | Coherent handshake, stale-continuation rejection, durable coverage, receipt retry, offline persistence and multi-tab tests pass. |
| 4 | Switch typing to prepare/commit/install/broadcast after package 3's capacity assessment and steps 1–3. | No source broadcast precedes commit; database outage/restart tests preserve local work. Remove old source autosave and competing-writer reconciliation. |
| 5 | Complete browser/server protocol compatibility without a shared WASM implementation. | Existing browser integration and serialized cross-platform fixtures pass; server builds remain native-only. |
| 6 | Complete durable revision/evidence mappings, cold anchor resolution, and asynchronous explicit archives (Stage F). | Migrations audit existing anchors/checkpoints, preserve exceptions, and demonstrate cache-independent recovery. |
| 7 | Integrate, repeat representative performance/failure checks, complete the deletion ledger and rehearse cutover/rollback (Stage G). | Section 28 is satisfied with executable local evidence; any outstanding production validation is explicitly listed. |

Keep rendering client-side throughout every step. Source projections and source archives do not authorize adding server rendering or compilation.

Resolve routine implementation details autonomously within these requirements. Missing production credentials, provider pricing inputs, or deployment approval need not stop local implementation and tests; retain explicit production acceptance items instead of inventing results. A failed architectural gate requires evidence and a documented design revision before dependent work, while independent work continues. Do not silently relax invariants, add a permanent fallback architecture, or declare the entire redesign complete because the first three packages pass. The final handoff must state implemented behavior, validation performed, removed paths, migration/rollback instructions, and any remaining unmet criteria.

### 24.1 General rules

Ship the redesign incrementally in development, but maintain only one authoritative production write path per document. No dual-writing source to old and new engines and no bidirectional bridge between independently mutable rooms.

Migration preserves immutable document IDs, Loro peer/operation identities, source content, proposals, review provenance, and offline caches. Do not transform history into plain text and seed a fresh graph as a convenient migration.

Existing worktrees may contain concurrent edits. Re-resolve current call sites and schema immediately before each implementation stage. The locations in this spec are starting points, not an instruction to restore deleted code.

Stage letters organize work; they are not a requirement to complete the entire browser core migration before improving server correctness. Stages C and D establish server ownership and the durable transaction boundary; Stage E establishes browser lifecycle ownership. Stage B consolidates semantics and can proceed independently once its prototype passes. Stage F completes revision/evidence guarantees beyond the frontier-based comments already implemented. Stage G verifies deletion and cutover across those streams. Coordinate protocol prerequisites before switching production typing.

### 24.2 Stage A: evidence and executable contract

Inventory every source mutation caller, every direct room-state access, every transaction that writes a proposal/comment alongside source, and every browser lifecycle owner. Record mutation and lock diagrams from code.

Create representative fixtures and benchmark the existing system: active small paper, long-history paper, near-limit project, many files/assets, many comments, simultaneous editors, offline reconnect, and agent proposal review.

Implement failure-injection infrastructure for transaction commit, response loss, ownership loss, and local persistence. Prefer testing public behavior with real Loro and PostgreSQL over source-text assertions.

Complete the single-document browser prototype in section 10.1 and a representative candidate/commit benchmark under section 27 before their dependent migrations. Record findings and sizing for each stage: affected callers and schema, expected implementation effort as a range with assumptions, dependencies, independently demonstrable benefit, and deletion/rollback boundary. Do not imply that all stages have equal certainty or must ship as one rewrite.

### 24.3 Stage B: native core and protocol compatibility

Extract the core without changing stored graph identity. Establish one browser Loro instance and CodeMirror integration. Move directory rules, normalized candidate validation, source projection, and hunk rules behind its API.

Do not begin a Rust/WASM browser migration. Keep the native authoritative validator independently reviewable from the existing browser integration and test their boundary with serialized fixtures.

Delete duplicate helpers as callers migrate. Keep wire compatibility at the edge while behavior is compared with fixtures. This stage must not create a permanent legacy core.

### 24.4 Stage C: transaction boundary

Refactor storage methods to participate in one caller-owned transaction. Add source revision metadata and semantic receipts. Implement candidate preparation with no live mutation and a commit-store entry point.

Prove source-plus-proposal/comment atomicity and uncertain-commit recovery before moving the normal typing path.

### 24.5 Stage D: document owner and writer fencing

Introduce deployment writer ownership and route all mutating adapters through one document task. Replace registry admission and remove direct mutable-state access.

Switch ordinary source batches to commit-before-install/broadcast after the early capacity/latency gate selects a configuration and compatible client behavior is established. Remove server autosave and competing-writer reconciliation after all mutation routes obey ownership. Do not leave an old `receive_update` bypass for one integration.

### 24.6 Stage E: client lifecycle and protocol

Introduce protocol capability negotiation, immutable transfer baselines, persisted confirmation coverage, receipt reconciliation, and one session lifecycle. Migrate browser persistence without dropping unsynced updates.

Old clients that cannot satisfy the new protocol receive an upgrade-required response before sending mutations. They must still be able to recover/export local work. A temporary adapter may translate safe old requests, but it cannot advertise old visibility/save semantics.

### 24.7 Stage F: revision-backed review and archives

Backfill source revision/frontier mappings while legacy update rows are still available. Map existing checkpoints and anchors to exact recoverable frontiers. Preserve legacy archives as evidence and export artifacts.

Preserve the existing frontier-based comment path without reintroducing checkpoint creation. Establish that referenced source is durable atomically with the comment, migrate immutable anchor evidence, and verify cold resolution. Move explicit checkpoint archives to asynchronous materialization, then remove remaining durable attachment writes. Frontier-based comments alone do not establish completion of this stage.

If a legacy row cannot be mapped exactly, retain its original archive and evidence and record an explicit migration exception. Do not guess a nearby revision. Cutover reports must count and explain all exceptions.

### 24.8 Stage G: deletion and cutover

Remove retired schema fields only after reads, writes, workers, backups, CLI, and migrations no longer reference them. Regenerate SQL metadata. Remove compatibility adapters after the declared upgrade window.

Before production cutover: create and verify a recoverable backup, drain old writers and active commits, verify schema and ownership configuration, deploy compatible clients/server, and run recovery and smoke checks. Do not implement destructive cutover as part of writing this specification.

### 24.9 Rollback boundaries

Before new-format writes, rollback can use the previous application with additive schema still present. After new-format writes or anchor migration, an old binary must refuse startup unless proven compatible. Do not promise binary downgrade after irreversible data changes.

Use a tested forward repair or restore a verified backup with explicit acknowledgement of the writes that would be lost. Preserve browser local state so it can be inspected and reconciled after recovery. Every migration stage declares its last safe downgrade point.

## 25. Required deletions and simplification ledger

Maintain a checked-in implementation ledger linking each replacement to deleted code and tests. Renaming a subsystem does not count as deleting its responsibility.

| Current responsibility/location | Target disposition | Proof required before deletion |
| --- | --- | --- |
| Public `Room.state` and scattered mutation locks | Private state inside document task | All production mutation callers use commands |
| `session_write`, `bundle_write`, `bundle_checkpoint`, restore coordination | Owner ordering and candidate transaction | Failure tests show no exposed transient state |
| `dirty`, dirty-age/save-floor tracking and source flush sweeps | Installed source is durable | No adapter relays before commit |
| Per-socket sent/acked high-water persistence tracking | Committed source coverage and semantic receipts | Lost-response/restart tests |
| Active `merge_remote_updates` reconciliation | Enforced single writer | Epoch-loss/takeover tests |
| `rollback_bundle_memory` and compensating live edits | Discard uncommitted candidate | Restore/proposal/storage-failure tests |
| `decode_update`/`DecodedAdmission` redundant staging | One prepared candidate validator | Malformed/limit/dependency tests |
| Duplicate browser directory helpers | Shared core API | No direct bypass callers, including tests/tools |
| Live-frontier comment references (checkpoint-per-comment already removed) | Committed source revision reference with atomic dependencies | Source/comment failure tests and historical anchor evidence audit |
| `pending_attachments`, `flush_attachments` | Disposable revision-keyed resolution | Cold/warm resolution equivalence |
| Separate reader/project/socket reconnect orchestration | One project lifecycle | Stale-continuation and teardown tests |
| Unbounded duplicated pending update byte arrays | Local CRDT plus durable coverage | Offline retry/restart tests |
| Infallible/compatibility uncached room loading | One fallible registry admission | Concurrent load/cancellation/capacity tests |
| Giant optional message and parallel command conversion | Typed envelope and payload | Protocol upgrade/invalid-field tests |

Do not delete storage integrity checks, transactional quota accounting, immutable blob GC, authorization rechecks, or offline recovery to make the ledger look smaller. Those responsibilities remain necessary under the target model.

## 26. Verification matrix

### 26.1 Core and interoperability

- Native validation accepts valid serialized browser operation sequences and produces the expected source trees, stable IDs, normalized paths, review hunks, and anchors.
- Astral Unicode, combining marks, normalized/case-folded paths, empty projects, missing main files, and very long valid names are covered.
- Concurrent same-path creation with preexisting suffixed destinations preserves every file and normalizes idempotently.
- Rename/edit, delete/edit, simultaneous main-file changes, file/asset collisions, and multi-file restore have explicit outcomes.
- Invalid roots/container types, deleted hidden payloads, malformed bytes, missing causal dependencies, and compressed input bombs are refused within bounded resources.
- Candidate rejection changes neither committed source nor its frontier. Final limits include normalization.
- Partial proposal acceptance preserves original operation authorship and applies exactly the reviewed subset on both platforms.

### 26.2 Database and ownership

- Crash before transaction, during transaction, after commit before installation, and after installation before response.
- Source and related proposal/comment rows either both commit or neither does.
- Duplicate semantic request with identical payload returns the original result; different payload under the same ID is refused.
- Two backend processes contend for ownership; only one is ready and can commit.
- Ownership connection disappears while pooled transaction connections remain alive; stale epoch cannot commit after takeover.
- Grant revocation races a prepared batch; revoked contributions do not enter committed source.
- Compaction races an append and owner restart; every acknowledged update remains recoverable.
- Full history export below its ceiling is not refused because a sum of old delta lengths exceeds that ceiling.
- Read after a returned receipt sees that commit or waits; a normal concurrent read is a coherent committed revision.

### 26.3 Browser and transport

- Hydration, metadata lookup, baseline fetch, and receipt lookup complete after disconnect, replacement connection, and disposal; stale continuations do nothing.
- Closing a session removes actual library-owned presence timers and subscriptions, not just injected test timers.
- Two tabs edit and compact IndexedDB concurrently, then both restart offline without losing updates or falsely confirming each other's work.
- A prepared project opens and edits with all server requests failing, then reconnects after remote concurrent edits.
- Drop the socket before commit and after commit before receipt; local work and semantic request identity survive restart.
- A slow baseline transfer races source commits; no update falls between baseline and subscription.
- Slow subscriber queues overflow without blocking other users or the document transaction; reconnect restores coherent state.
- Revoked access and recreated slugs never receive old local updates. Recovery/export remains available.
- A local persistence failure, remote quota refusal, unsupported schema, or browser upgrade never produces a false synced indicator.

### 26.4 Review, reading, and archives

- Creating comments writes no source archive and creates no unnecessary source revision.
- Clear attachment caches, restart, and resolve every fixture; original target identity is preserved.
- A comment from a stale or uncommitted render preserves its draft and cannot attach to unrelated source.
- Non-editors cannot fetch editor transfer references, proposal branches, deleted asset versions, or historical source through event replay.
- Archive generation at revision R races edits to R+1; the artifact matches R exactly and has verified asset references.
- Failed archive jobs do not make committed source unavailable and do not create repeated checkpoints.
- Restore precondition failure changes nothing; successful restore source and labels appear atomically.
- Backup/restore revalidates old client coverage instead of trusting receipts from a newer lost server state.

### 26.5 Migration

- Migrate real old databases and browser caches, including pending edits, open proposals, legacy anchors, and missing archive jobs.
- Compare source trees and operation identities before/after migration, not only displayed text.
- Audit every legacy anchor/checkpoint mapping and count unresolved exceptions.
- An incompatible old binary refuses to write new-format data.
- Exercise the documented downgrade boundary and a full restore from the verified backup.

## 27. Performance and cost gates

Record baseline and target results on the same hardware/database configuration and fixtures. Report distributions rather than one favorable run.

Measure at least:

| Workload | Required observations |
| --- | --- |
| Single editor, continuous typing | Local input latency, batching delay, commit latency, remote visibility latency |
| Several simultaneous editors | Queue delay, rejected/rebuilt batches, throughput, convergence |
| Large history with small visible source | Candidate preparation CPU, export work, memory peak, commit bytes |
| Many comments | Source commit latency independent of eager reattachment, cold attachment resolution time |
| Many idle documents | Database query rate, task memory, eviction behavior |
| Large offline reconnect | Transfer bytes, missing-dependency handling, bounded memory, time to durable confirmation |
| Explicit checkpoint/export | Synchronous transaction cost, archive latency, deduplicated bytes |
| Database outage and recovery | Local continuity, queue bounds, recovery latency, retained work |

Record source transactions per active minute, WAL bytes, object writes, compaction frequency, and storage growth. Credit only incremental reductions relative to the current tree; checkpoint-per-comment and publication removal are already in the baseline.

Before the Stage D typing switch, compare commit-before-relay at 500 ms, 1 second, and 2 seconds against the current batched-persistence baseline. Include small visible source with long history so full candidate preparation and encoded-history measurement cannot hide behind a small-paper benchmark. Report collaborator visibility latency distributions, throughput, peak memory, and database demand at representative simultaneous active-document counts; do not equate account count with continuously active documents.

Translate measurements into a monthly cost estimate using the intended provider and a stated editing-hours/activity model. Separate fixed provisioned capacity, usage-dependent charges, and capacity upgrades. Include a managed database network path representative of deployment. Measure idle query activity and assess ownership-connection behavior before making scale-to-zero claims. The benchmark's purpose is to validate capacity and select the batching interval for the preferred commit-before-relay target, not to assume that higher transaction counts require higher bills.

Set numerical latency, capacity, and cost acceptance thresholds from the baseline and deployment budget in Stage A, before evaluating the target results. Record an explicit proceed/revise decision before the typing switch. Repeat representative checks on the integrated implementation before production cutover. If no tested configuration is usable and affordable, revise the design and its affected invariants rather than silently introducing a second commit model.

If deployment inputs are unavailable during implementation, state provisional local latency/capacity targets before the target benchmark and provide a cost model parameterized by provider capacity and editing activity. Use those results to exercise the new typing path locally and finish integration; keep production affordability and managed-network validation explicitly pending. Provisional targets are not acceptance of a production budget. Missing deployment inputs alone do not require preserving the old implementation as a permanent fallback.

No architectural complexity may be reintroduced solely to improve a synthetic benchmark without showing the affected real workload and comparing the simpler option. In particular, do not restore independent live/durable source states to hide commit latency.

## 28. Implementation boundaries and completion criteria

This project is complete only when:

1. Every source mutation and related review decision enters one document owner and durable transaction path.
2. Authoritative state is installed and broadcast only after confirmed commit.
3. Single-writer enforcement survives connection loss and process takeover.
4. The server uses the native authoritative core, the browser retains one existing JavaScript Loro document, and serialized interoperability fixtures pass without any server-side browser/WASM compilation.
5. Offline text and file editing, local persistence, refusal recovery, and multi-tab behavior pass real-browser tests.
6. Comments and checkpoints reference committed source revisions, with historical mappings audited.
7. Derived attachments are reconstructible without their cache; ordinary comments create no archives.
8. The simplification ledger's replaced code paths are deleted, not left behind as alternate modes.
9. Existing source/history/proposal/asset security boundaries pass regression tests under current-source viewing.
10. Performance and database-cost results are recorded, compared with baseline, and accepted before deployment.
11. Backups, recovery, migrations, protocol upgrades, and downgrade boundaries have executable evidence.

The largest expected gain is fewer independently advancing states: one committed server document, one local working document per client session, and explicit immutable revisions between them. Preserve that boundary throughout implementation. A design that recreates speculative server rooms, independent save loops, or several mutation owners under new names does not meet this specification.
