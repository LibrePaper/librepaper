> **Superseded.** This describes a SQLite catalog v2 that was never shipped.
> The storage that ships is PostgreSQL catalog v3 — see
> [catalog-v3-postgres.md](catalog-v3-postgres.md), which starts from the
> product requirements rather than preserving this schema. The migrations in
> `crates/librepaper/migrations/postgres/` are the only normative DDL. Keep
> this file for the constraint reasoning behind it, not as an implementation
> target.

**Catalog v2: migration to twelve tables**

Status: proposed implementation specification. Date: 2026-09-12. This document supersedes the table-preserving recommendations in `docs/sql-schema-review.md` where they conflict. The objective is a substantially simpler storage model for unreleased software, accepting explicit breaking changes. It is not a request to implement forty-seven logical tables inside JSON columns.

Revision: incorporates the subsequent schema review. Twelve tables remain the target; the changes add enforceable local constraints, bounded accounting/reference counters, and explicit transaction contracts rather than more tables.

The accompanying [catalog-v2.sql](catalog-v2.sql) is the normative proposed DDL. It is deliberately outside the application's migrations directory. This specification, that DDL, and the acceptance scenarios together define the implementation target. The existing application has not been migrated by writing these files.

**1. Outcome and boundaries**

The completed application shall have exactly twelve persistent application tables, no policy triggers, no runtime persistent DDL, and one SQLite catalog per deployment:

| Table | Owns |
|---|---|
| `accounts` | Registered identities and the small set of anonymous/system ownership accounts; preferences and owner counters |
| `documents` | Stable document identity, owner, configuration, current source/publication pointers, document counters |
| `grants` | One effective named grant per account/document |
| `links` | Independently identified share credentials, currently one per role/document |
| `annotations` | Comments, highlights, source suggestions, selectors and resolution |
| `replies` | Discussion replies |
| `checkpoints` | Snapshot event metadata and protection eligibility |
| `objects` | Every physical application object, its measured size or allocation reservation, and its lifecycle |
| `checkpoint_objects` | Complete flattened physical dependency set of each checkpoint |
| `object_leases` | Temporary read/write/staging protection for physical objects |
| `operations` | Explicitly supported recoverable operations, execution leases, and bounded replay receipts |
| `server_state` | A singleton containing deployment identity, generation, counters, key metadata, and meter state |

SQLite internal tables are excluded from this count. There shall be no extra durable tables for source encodings, journal coverage, retention candidates, rate buckets, key rotation metadata, cancellation flags, or derived result formats. There shall be no generic entities/attributes table, generic graph engine, persisted workflow DAG, or unrestricted key/value store. Ordinary views are permitted only when they express a named reusable query and are declared centrally; none is required by the baseline.

Retain the real product: source editing, collaboration, immutable history, explicit rendered publication, all five source formats, sharing, suggestions, agent workflows, ownership transfer, account erasure, local deployment, and recoverable backups. Retain compression and within-document content reuse. The production storage implementation currently present is `FsStore`; the migration must first work with its actual durability and task-lifetime behavior. Support for an arbitrary future remote blob adapter is not a prerequisite and must not be claimed without validating its I/O settlement contract.

Do not introduce distributed SQL, multiple active deployment writers, cross-document deduplication, multiple provider identities per account, organizations, or a configurable workflow platform during this migration.

**2. Deliberate behavior changes**

These changes are part of the design, rather than accidental losses discovered during implementation:

| Area | v2 decision |
|---|---|
| Storage charges | Charge measured object bytes and explicit outstanding allocations. Do not charge each SQL field or index to a customer |
| Journal | Segments and bases belong to exactly one document; no shared segment billing or coverage transfers |
| Object writes | Immutable physical versions; change pointers rather than overwrite a live physical key |
| Grant roles | A single effective `reader`, `commenter`, or `editor` role per account/document |
| Guest pins | Bounded account bookmarks, which never independently confer access |
| Retention | A small per-document policy, current protection checks, grace on checkpoint rows; no persistent preview candidate list |
| Retention preview | Advisory and time-sensitive. Execution can retain a candidate whose protection changed; no promise of an identical future deletion set |
| Storage pressure | Reject new allocations when hard limits are reached. Never silently delete protected history to make an upload fit |
| History usage display | Report stored bytes and logical history separately. Do not present a reconstructed, exact live/history/publication split as authoritative billing |
| Rate limiting | Bounded in-memory token buckets; process restart resets short-window rate state |
| Retry keys | Time-bounded v2 keys and finite receipts; an expired forgotten key is rejected, never re-executed |
| Old publications | Preserve current rendered publication. Historical publication IDs remain provenance, not a guarantee that old rendered bundles stay downloadable |
| Historical parents | Preserve original event parent identifiers; indicate missing ancestors at read time rather than rewrite retained ancestry |
| Source mutation contention | One in-flight source writer per document. Competing mutations receive a typed busy/conflict response; display staging and ordinary annotation writes remain independent |
| Existing databases | No in-place rewrite of a live v1 database. Fresh v2 root or offline conversion into a separate root |

All protocol/manual/UI changes necessary to communicate these decisions are implementation work, not follow-up suggestions. In particular, update `docs/protocol/publication.md`, quota pages, preview/apply wording, and CLI error handling.

**3. Identity, types, and time**

`documents.id` is the old immutable `storage_id`, when converting. `slug` is a unique public address. Every relational document reference uses `document_id`; SQL shall never join through a slug to reach object ownership. Object paths must not depend on a mutable slug. Slug changes are permitted by the model; adding a rename endpoint is outside scope.

New object and operation IDs are random 128-bit identifiers encoded as lowercase hexadecimal. Checkpoint IDs identify events, not content. New checkpoint events use random IDs; the converter may preserve nonempty legacy event IDs. Existing API validators that assume every revision/event ID has 64 characters must be changed. Digests are lowercase SHA-256 hex; the application validates shape in addition to SQL length checks. Cross-document references use composite foreign keys and fail even when two documents happen to reuse an event or object ID.

Use Unix milliseconds in all SQL time columns. Wire responses continue to use RFC3339 where appropriate, with conversion in one utility module. NULL means absent. Empty strings and `-1` shall not mean “no expiry,” “no parent,” “no publication,” or “allocate sequence” in persisted state. Empty body/label display values remain legitimate where specified. Logical sequence allocation is independent of clocks.

All twelve tables are STRICT. Small keyed relational tables use WITHOUT ROWID; `server_state` uses its integer singleton key. SQLite CHECK/FK/UNIQUE constraints establish local invariants. The catalog transaction API establishes root completeness, authorization, counters, and lifecycle transitions that cannot be expressed as simple row constraints. Do not add policy triggers to bridge that distinction.

Every writer computes nondecreasing row `updated_at`, at least its `created_at`, even if wall time moves backwards. Expiry limits admission; it is never evidence that an in-flight physical write has stopped. On restart, a new writer generation invalidates process-bound execution authority independently of clocks.

Every JSON payload has an integer `version` and a fixed Rust type. `json_valid` establishes valid JSON only; application decoding must also enforce object shape, version, limits, and field semantics. Preserve unknown preference fields within the payload size limit; reject unknown recovery-plan versions. Never use JSON contents as the only durable record of an object reference whose liveness GC must understand.

**3a. Enforcement boundary**

The DDL is not a proof that arbitrary direct SQL mutations are safe. Every invariant has an explicit primary enforcer:

| Invariant | Primary enforcement | Independent verification |
|---|---|---|
| Types, non-null keys, local state shape, role/kind domains, same-document references | STRICT, CHECK, UNIQUE and composite FKs | DDL validator |
| Kind/scope matrix, terminal receipt deadlines, deadlines for expiring prepared kinds | CHECK constraints | DDL validator exercises every kind/scope and deadline class |
| One prepared erasure per scope, one source writer, one execution epoch per conversation | Partial unique indexes | Valid concurrent-operation fixtures plus duplicate rejection |
| Available object has measured length, no reservation and no allocation owner | CHECK constraints | Settlement/cleanup transaction tests |
| Complete checkpoint dependency set, correct tree kind, all dependencies available and verified | Typed checkpoint commit: verify closure while protected, then recheck states and commit all edges in one immediate transaction | Manifest/SQL closure audit and concurrent create/delete tests |
| No new lease/reference on an object claimed for deletion | Conditional insertion/state checks inside the same immediate transaction as lease/reference acquisition | Lease-versus-GC and checkpoint-versus-GC race tests |
| Charge survives physical write/delete uncertainty | Typed object transition methods retain the prior charge until confirmation | Fault-injection tests and independent physical inventory reconciliation |
| Cached byte/count values equal their defining rows | Typed mutation methods update rows and counters together | Recompute-and-compare verifier, never ordinary admission |
| Reference/staging admission stays below limits | Counter CHECK ceilings plus conditional counter updates with configured limits | Limit-boundary, rollback, and concurrent admission tests |
| Current authority and source/publication generation remain valid | Commit-time authorization and compare-and-swap | Revoke/erase/cancel races |

Flattening checkpoint dependencies simplifies GC and makes ordinary FKs possible; it does not prove set completeness or physical readability. SQL alone permits an incomplete checkpoint, an edge to an allocated/deleting object, or a lease on a deleting object. Those are forbidden through the application API. The schema validator records these SQL-permitted boundary cases explicitly rather than pretending they are database guarantees. The application transaction tests are the preventative defense; reconciliation is a separate detector, not a substitute.

Remove public arbitrary-connection mutation access from normal production callers. Source, room, agent, maintenance, and importer code must use the typed mutation boundary. Repairs that bypass it require exclusive offline ownership and a full invariant audit before reopening. No policy triggers are introduced to make these cross-table/set claims appear local.

**4. Accounts and ownership**

Every document has one non-null owner account. This avoids a second quota/authorization algorithm for `owner_key` versus `owner_id`.

An account is one of:

- `registered`: a provider/subject identity with normal authentication.
- `anonymous`: a durable anonymous publishing owner, created only when an anonymous owner actually owns content. It is not created for every viewer or request. Its session capability is generated independently of its public ID.
- `system`: a deployment-created non-login account for examples or explicitly open/unowned content.

This is a narrow ownership model, not a universal principals abstraction. Account kind never grants a visitor permission. Open publishing is an explicit document mode plus deployment policy. Requests cannot claim an owner identity by sending the system account ID or a historical sentinel such as `unowned`.

Registered provider and subject are stored separately with a unique pair. Existing provider-qualified account IDs may remain the internal account IDs during conversion. Handle and display name are mutable labels. Email is nullable and is never an identity merge key. Linking multiple providers to one account is outside v2.

`preferences_json` contains the simplified defaults, display timezone, warning preferences, and other small account settings. `preferences_revision` provides optimistic concurrency. Per-document overrides move to the document that owns them. Large per-document maps must not accumulate in account preferences.

`bookmarks_json` has shape `{version:1, items:[{document_id, link_id, credential_generation, pinned_at}]}`. Limit 1,000 items and 256 KiB, whichever is reached first. Reading the landing page collects at most the bounded bookmarked IDs and joins/filter-checks actual document and link access. A stale bookmark is omitted and lazily removed. It must never be transformed into a durable editor grant. Named grants are listed from `grants`, not duplicated into bookmarks.

`onboarding_json` contains at most 16 named example slots, each with a deterministic target document ID/slug and state `pending` or `complete`. Allocate slots in the transaction that creates the account. Mark a slot complete only after its target document is active. A deleted completed example is not recreated on profile refresh. A reserved slug is not yet a document reference and intentionally has no FK. Concurrent onboarding retries must reuse the same target identity.

Owner counters include creating, active, and deleting documents. Transfer uses one immediate transaction to recheck authority and destination quota, move the entire document's stored/reserved totals and document count between accounts, and change `owner_id`. It must also move process-local edit reservations under the shared admission lock. Deployment totals do not change. In-flight allocation completion reads the current owner from `documents`; it must not charge a stale owner captured before object I/O.

**5. Document fields and revision semantics**

`title_key` is computed centrally as NFC normalization, trim, then Unicode lowercase. It preserves the present intention of case-insensitive owner-local title uniqueness without asking SQLite NOCASE to implement Unicode. It is persisted and indexed; title changes and ownership transfer recompute/revalidate it. Conflicts during import are reported rather than silently renaming user documents.

Title and slug release deliberately differ. Marking a document deleting frees its owner-local title so another document with the same display name can be created. Its public slug remains reserved until physical teardown finishes, avoiding reuse of an address whose deletion/retry work is still outstanding. Neither identity is inferred from the other; test both behaviors.

`source_format` is one of markdown/html/typst/latex/quarto. `main_path` is a normalized relative path with no absolute prefix, traversal, NUL, or empty component. Execution engine is derived from source format where the product currently makes it a function of that format; there is no derived SQL table.

`settings_json` stores bounded configuration the database does not query: compile settings and document policy details. Settings are versioned separately from the source generation. Persist typed columns for database-owned identities, permission facts, indexed predicates, uniqueness, and worker selection. This rule does not require copying deployment configuration or every derived policy value into SQL. Do not put owner/status/head pointers in settings JSON.

`source_generation` advances on every committed durable source-state transition. It is the compare-and-swap token for captured source work. `journal_sequence` is monotonic within one document and never resets when `journal_epoch` changes. Epoch changes identify a new compacted lineage; checkpoint protection and recovery compare the explicit epoch/sequence contract, not a deployment-wide MAX across unrelated documents.

`current_checkpoint_id` is the most recent explicitly committed checkpoint, not necessarily the latest acknowledged live edit. `journal_base_object_id` and its sequence plus subsequent available live journal segments reconstruct current durable state. `publication_id` is the activated display event; `publication_object_id` points to its manifest. A source checkpoint does not activate a rendered publication.

`next_checkpoint_seq` and `next_annotation_seq` allocate positive sequences with transaction-local increments. Never use `MAX(seq)+1`: removing old rows must not make an event cursor reusable. Sequence numbers may have gaps and are never renumbered after retention.

There is no `pending_publication` JSON blob on the document. Prepared work is in `operations`; the committed document remains readable while a replacement is prepared. A new document remains `creating` until its required source/publication transaction commits. Only active documents are served normally.

**6. Sharing and attribution**

Keep the existing role ladder: reader < commenter < editor < owner. Ownership exists only on documents. Grants and links cannot grant owner. Collapse multiple legacy grant rows to their strongest effective role during conversion. Removing a v2 grant removes that one effective grant; no hidden lower role remains.

Links have stable IDs, but v2 retains one link per role/document through a unique constraint. This preserves the existing share UI rather than introducing multi-invitation behavior as part of a database migration. Rotating a token increments `credential_generation`, changes hash/sealed token atomically, and invalidates bookmarks bound to the old generation. Do not change link ID for a mere label edit.

Keep digest uniqueness and authenticated encryption with document ID, role, and token digest bound into associated data. `sealing_key_id` names metadata in the bounded external-keyring configuration reflected in `server_state`; it is not a SQL secret. Current display capabilities continue to recheck live account/link authority on every request, including cached/conditional responses.

The one `active_link_key_id` in `server_state` is authoritative. `keyring_json` contains at most 32 known key descriptors, not secret material or an independent primary flag. `rotate_links` is a server-scoped operation with a cursor `(document_id, link_id)`. It reseals at most 200 links per transaction. Keep source and destination secrets available until all rows are converted and verified. New link writes use the designated destination according to the operation contract; after the last batch, atomically switch the singleton designation and commit the operation. The no-links case must perform the same final designation update. Recovery never elects a primary by timestamp ordering.

Annotation/reply `author_account_id` carries authoritative registered attribution when available. `author_key` is a server-derived actor identity for ownership of anonymous annotations and operation replay; it contains no raw credential. Display names are separate. An account ID sent by a client is insufficient to establish attribution. `ON DELETE RESTRICT` ensures erasure must process attribution explicitly instead of silently leaving a display name behind.

**7. Annotations and suggestions**

Keep one annotation table with a small explicit kind and optional suggestion fields. Do not split each selector subtype into another table.

`selector_json` version 1 contains the rendered target and optional source target. Allowed rendered kinds are text quote/position, point, and figure region. It preserves exact/prefix/suffix, position, region coordinates, and optional rendered-output path where applicable. A source target contains relative path, exact/prefix/suffix, position, and captured source identity. `context_json` contains bounded presentation/execution metadata such as color, review pass, and Quarto output context. Values used for retention or authorization stay in typed columns.

Validate nonnegative positions, valid finite region coordinates, normalized paths, and internally coherent selector kinds. An annotation may preserve a historical `publication_id` or `source_revision` after the corresponding artifact is gone; these are intentionally not live-row FKs.

`protected_checkpoint_id` is different: it is a real retained-checkpoint FK for an unresolved source-dependent annotation. Creating such an annotation must identify a captured checkpoint of its source revision, creating that checkpoint first through the normal write protocol when necessary. A historical imported annotation whose referenced source no longer exists may retain provenance with no protection pointer, but must be marked unavailable in its selector/context and cannot be accepted as though its base still existed.

Resolution clears the protection pointer in the same transaction as `resolved_at`. Reopening a source-dependent annotation must reacquire a valid protected checkpoint or fail explicitly. Labels and unresolved annotation pointers protect checkpoints from automatic retention.

A suggestion is proposed, accepted, or rejected. Transient merge/conflict errors are operation results; the suggestion remains proposed unless an actual terminal decision commits. Acceptance atomically commits the source checkpoint/head transition, the accepted state, resolution revision/time, and operation receipt. The accepted operation ID is historical provenance and intentionally not an FK to a receipt that expires. A receipt's expiry must not erase the annotation's accepted state.

Default bounds: 1,000 annotations/document, 100 replies/thread, 64 KiB body, 64 KiB selectors, 16 KiB context, and 1 MiB proposed replacement text. Preserve stricter existing request limits when applicable. Enforce aggregate counts transactionally. These are row-count/payload bounds, not simulated SQL byte billing.

Deleting an annotation continues to delete its replies. Account erasure must preserve that existing behavior and document it: removing a user's parent annotation also removes its discussion thread. Changing that product rule is a separate change.

**8. Physical object model and immutable manifests**

Every application-managed physical payload in the primary object namespace has exactly one `objects` row before a PUT starts. Metadata backups, external secrets, and SQLite files are separate deployment overhead. No unregistered application PUT is permitted.

New storage keys are `v2/documents/<document_id>/objects/<random_object_id>`. A key is never reused, including after deletion. A digest is content identity; a random object ID is physical allocation identity. The same bytes may exist as a replacement allocation while an old allocation is being retired. Reads follow immutable manifest object IDs, not whichever currently available object happens to have the same digest.

Within a document, writers may reuse an available object with matching kind, digest, and encoding version after acquiring protection atomically. They may not reuse an allocated or deleting object. Races that cause two physical copies of the same bytes are safe and fully charged; deduplication is an optimization, not an integrity requirement. There is no cross-document physical reuse.

Source recipes and trees become versioned immutable envelopes that contain explicit physical object IDs plus integrity digests. A source recipe retains chunk order, offsets, logical file digest, codec/profile version, and uncompressed length. A tree retains file paths, file identities, logical digests, main path, and settings. Store physical locators separately within that envelope from the canonical logical tree representation. A tree's logical digest is not the digest of its locator-containing encoded envelope. Do not leak account identity into immutable source manifests merely to simplify SQL attribution.

`objects.digest` hashes exact stored bytes. `logical_digest` is optional lookup metadata for reusable source recipes/files; it is not another authoritative object size or ownership fact. `encoding_version` identifies the physical representation contract. Retain the current compression/chunking algorithms initially; change their envelope format without mixing in an unrelated compression experiment.

Object states:

| State | Meaning | Accounting |
|---|---|---|
| `allocated` | Capacity admitted; write absent, ongoing, or awaiting durable settlement | `reserved_bytes`; measured stored length is NULL |
| `available` | Payload integrity and durability verified; may be referenced | Exact `byte_length`; reservation is zero |
| `deleting` | New references are forbidden; physical reclamation is pending/retrying | Known length or the still-unresolved allocation reservation remains charged |

Available does not itself mean visible to readers: serving requires a valid document/publication/checkpoint root and request authorization. Availability is never inferred from the presence of a file alone.

`allocation_operation_id` exists only while physical length/settlement is unknown. The settlement transaction clears it when setting measured `byte_length`, including a confirmed empty object of length zero. If activation is still pending, `object_leases.operation_id` retains the staging relationship. Available objects must not pin old allocation receipts. An unknown-length deleting row must still name its allocation owner, even when the admitted write was zero bytes. These row checks establish a valid representation, not that a writer preserved yesterday's charge: transition methods must preserve it exactly until settlement.

`live_root` protects a physical object required by acknowledged live state, including dependencies named by live journal entries. `publication_root` protects every physical object in the current rendered bundle. These are concrete root sets, not arbitrary bit flags for future subsystems. There is no live reference hidden only inside an unparsed operation plan.

Available objects referenced by checkpoints are protected through `checkpoint_objects`. Reader and staging protection uses `object_leases`. A backup temporarily freezes destructive object reclamation through one explicit prepared backup operation. That is the full root vocabulary.

**9. Complete checkpoint closure**

Each checkpoint's `checkpoint_objects` rows enumerate every physical object needed to open it: the tree envelope, recipes, chunks, and assets. The relation is flattened intentionally. Sharing a chunk across many checkpoints produces many small reference rows and only one charged object. This trades reference-row count for removing separate encoding/object/asset-set schemas and graph GC traversal.

The writer computes and verifies the full transitive closure outside the SQL transaction while holding read/write leases. It rejects cross-document IDs, missing dependencies, cycles in malformed envelopes, and digests that do not match. The commit inserts the checkpoint and complete closure atomically. Maximum 16,384 distinct physical objects/checkpoint in v2; reject a larger write before committing anything visible. No placeholder “incomplete checkpoint” state is supported.

The independent checkpoint-count and per-checkpoint limits are not sufficient: 4,096 × 16,384 permits 67,108,864 reference rows in one document. Add a cumulative ceiling of **1,048,576 checkpoint-object rows/document** and **8,388,608/deployment**. `documents.checkpoint_ref_count` and `server_state.checkpoint_ref_count` cache these actual row counts. These are initial implementation ceilings, subject to measured validation before release; configured limits may be lower. Raising a ceiling requires an explicit schema/spec change and new measurements, not an opaque preference override.

Count each distinct edge, including repeated references to the same physical object from different checkpoints. Identical checkpoints do not receive a reference-count discount. Check the document and deployment counters and increment them by the complete new edge count in the checkpoint commit transaction; decrement by the exact removed edge count in checkpoint deletion. Never maintain them from estimated logical size or distinct physical-object count. SQL constrains the cached values; the mutation API and independent audit establish equality to the relation.

Preflight obvious over-limit requests, but always repeat the limit check in the final transaction: another document can consume the deployment budget during encoding. A failed final admission rolls back checkpoint/edge/counter changes, leaves the old visible state intact, and safely reclaims staging through the ordinary operation protocol. It returns `checkpoint_reference_limit`, with scope and required/available counts. It does not evict labeled/protected checkpoints. Live journal appends that do not create a checkpoint remain admissible under their own limits; a compound operation requiring a checkpoint fails atomically.

The tree object's ID must appear in the closure, even though the checkpoint also has a direct tree FK. Before exposing the checkpoint, validate that all closure objects are available and protected by the writer's leases or already durable roots. Empty asset sets need no marker. A tree with no assets is simply a complete closure with no asset-kind entries.

`parent_id` remains an immutable historical event identifier, with no restrictive live FK. List/history APIs report whether the named parent is still present. They may offer nearest-retained comparison explicitly, but may not pretend that it was the original adjacent edit. Git commit, dirty state, and changed-path metadata move to versioned `metadata_json` because SQL does not use them as relational identities.

Checkpoint logical bytes are useful presentation metadata, never charged as physical bytes. Two events referring to the same tree retain different event IDs and sequence values.

**10. Accounting contract**

For a document D:

```text
stored(D)   = SUM(objects.byte_length for D where byte_length IS NOT NULL)
reserved(D) = SUM(objects.reserved_bytes for D)
charged(D)  = stored(D) + reserved(D)
admission(D)= charged(D) + process_local_edit_reservations(D)
```

The sums include objects in deleting state. Available objects have no reservation. An allocated object has no measured length yet. A deleting object carries one of these charges until physical settlement. There is no double charge from leases or checkpoint references.

Account stored/reserved counters equal the sum of document counters for that owner; deployment counters equal the sum of account counters. Document counts include every lifecycle state. All changes to object rows and the three levels of counters occur in the same immediate transaction. Use checked 64-bit arithmetic; reject overflow rather than saturating counters. Counters are permitted caches with an explicit derivation, not competing accounting models.

Agent staging has two additional subset counters on documents and the server singleton:

```text
agent_payload_bytes(D) = SUM(COALESCE(byte_length,0) + reserved_bytes
                             for objects of D with kind='agent_payload')
agent_payload_count(D) = COUNT(objects of D with kind='agent_payload')
```

Both include allocated, available, and deleting objects. The server counters sum the document values. They are subsets of ordinary object accounting, not additional billable bytes. Allocation increments their reserved byte contribution/count, settlement swaps to measured contribution without changing count, and confirmed deletion removes the remaining contribution/count. Expiry or a failed delete does not free staging capacity. Retries of an already registered identical allocation do not increment count again. Update these caches with the normal byte counters in the same transaction. Ownership transfer does not change document/deployment staging totals; ordinary owner totals still move.

Stage admission reads these indexed singleton/document rows, never SUMs over `objects`. A kind-leading index would improve a subset scan but would not satisfy this constant-size admission contract. Full subset SUM/COUNT queries belong only in reconciliation. Normal staged-object retrieval/cleanup is by operation leases and indexed object IDs.

Creation of an allocated object adds its upper-bound encoded allocation to all reserved counters. Successful settlement swaps that reservation for the measured size, atomically. The admitted bound must cover the write: if encoding needs more space, enlarge the reservation before starting the additional I/O, or abort. Removing a physical object decrements exactly its remaining charge once. Duplicate deletion confirmations find no row and change no counter.

Before replacing a 10 MiB live object with a new 12 MiB version, the application admits a new 12 MiB allocation while keeping the old 10 MiB charged. Activation does not refund the old bytes. Reclamation refunds them only after deletion confirmation. File-system scratch space and backups are additionally constrained by the existing physical free-space guard; logical counters do not claim to measure filesystem allocation precisely.

User hard quota and deployment object quota are tested from the maintained counters plus bounded live-edit reservations. The admission transaction must not union reference tables, scan historical objects, enumerate all accounts, classify paths, or fall back to “unknown accounting.” Full reconstruction exists only in an explicit verifier/reconciler, never in ordinary admission.

Hard byte limits are resolved from typed deployment configuration and `accounts.plan`; v2 does not support a customer-controlled hard-quota override in preferences JSON. Account preferences contain soft/user policy choices only. Commit-time admission resolves the current owner/plan under the shared admission boundary and uses a consistent configuration snapshot. If configuration hot reload is supported, fence the policy generation through that boundary so a stale request cannot commit under a replaced limit. A future administrator-owned per-account hard override would require a typed nullable SQL column and explicit authorization; it is not part of this migration.

Stop attributing each SQL field, JSON character, index, or page to account storage. Keep deployment catalog-file/WAL size metrics and a configured catalog limit, plus row/payload limits. Catalog pressure rejects new metadata growth while preserving reads, deletions, and bounded recovery work. Keep the existing filesystem maintenance floor and reserve a bounded cleanup margin; a full quota must not prevent deletion receipts and checkpoint-reference removal.

Unacknowledged room edits remain bounded in memory. Their reservation coordinator shares a serialization boundary with catalog admission, records per-document/per-owner/deployment totals, and provides rollback guards for failed SQL operations. Update both under that boundary and release only after commit/rollback resolves. No process-local reservation survives restart, and no acknowledged edit depends solely on such a reservation. Panic/cancellation tests must cover this boundary. A new TEMP table is not required by v2.

**11. Operations are a finite list of contracts**

An operation has exactly one scope: a document, an account, or the deployment when both references are NULL. Its payload is a versioned Rust enum selected by `kind`, not an arbitrary user-defined workflow. The catalog exposes typed methods rather than a public “execute plan JSON” endpoint.

The DDL enforces the complete kind/scope matrix below and the typed API mirrors it. `actor_key` is stable across ordinary reconnects for the same verified actor. Derive it from a registered/anonymous owner's stable identity or a server-verified visitor identity, never from a client-supplied account name. Anonymous visitors sharing a commenter link still have distinct visitor actor identities. Link/session authority is separately rechecked; an actor key is an idempotency partition, not a credential. The actor-leading operation index supports erasure of contributions outside an account's own documents.

Allowed kinds and completion conditions:

| Kind | Scope and durable completion |
|---|---|
| `source_publish` | Document; captured source/base/checkpoint state activated |
| `display_publish` | Document; expected prior publication replaced atomically |
| `checkpoint` | Document; checkpoint plus full object closure inserted |
| `checkpoint_delete` | Document; explicitly requested eligible checkpoint rows removed |
| `journal_append` | Document; complete segment set and sequence range committed |
| `journal_compact` | Document; new base and root set committed, old coverage retired |
| `agent_apply` | Document; one authorized source change and its receipt committed |
| `agent_annotations` | Document; annotation-only batch committed |
| `agent_cancel` | Document; cancellation decision/fence recorded |
| `agent_execution` | Document; active prepared row represents one fenced conversation epoch |
| `agent_stage` | Document; prepared row owns expiring staged agent payloads |
| `erase_account` | Account; attribution cleared and owned documents physically removed |
| `erase_document` | Document; completes as an internal task when its scope is physically removed |
| `rotate_links` | Deployment; every live link verified under destination key and singleton updated |
| `backup` | Deployment; snapshot and payloads verified and completion manifest durably written |

Only prepared, committed, and aborted are common states. Kind-specific progress is a small cursor or a bounded captured expectation in `plan_json`. Object sets live in leases or immutable manifests, not arrays that can grow past the 64 KiB plan limit. Results use a uniform JSON envelope `{version:1, outcome, ...}`; even hashes and refusal reasons live in that envelope. Do not copy the same result into a second cancellation/receipt table.

An `erase_document` row's scope FK prevents simply deleting the document. Its final transaction, after physical settlement and deletion preconditions, deletes that internal operation and any other terminal document receipts before deleting the document. The document then returns 404/410 according to the route; this operation does not promise a retained scoped receipt after its scope is gone. Apply the analogous rule to the final account-erasure operation. All other prepared work must have settled before this scope teardown.

Partial unique indexes permit at most one prepared `erase_account` per account and one prepared `erase_document` per document, independently of the caller's request key. A second erasure request returns the authorized existing progress/operation identity; it must not start a second cursor. The initial transaction marks the scope erasing/deleting and fences other writers. The unique indexes serialize erasure jobs, while lifecycle/authority checks prevent new non-erasure work.

Document writer exclusivity is enforced by the partial index for source publication, checkpoint creation, journal append/compaction, and agent source apply. Short operations recheck source generation; long I/O never holds the SQLite connection lock. A busy source slot returns a typed retryable response. Display publication permits up to two staged preparations per document, as the current protocol does; its cap is checked inside admission. It compares the expected publication ID at activation.

Execution epochs use `agent_execution` rows rather than a separate lease table. One prepared epoch per conversation is enforced by the index. Issuing a replacement first aborts the old epoch in the same transaction; renewals update expiry only if generation/epoch/state still match and the lease has not expired. Every source mutation checks the row at final commit. Keep the current 60-second execution lifetime initially. Process restart invalidates old epochs.

Agent staged content becomes ordinary `agent_payload` objects protected by stage leases and an `agent_stage` operation. Preserve current caps: 16 MiB/object, 32 MiB/document staged payload, 128 MiB/deployment staged payload, 512 staged objects/document, one-hour maximum stage lifetime. Add an explicit 16,384-object deployment staging ceiling to bound tiny/empty payload metadata. Use the typed subset counters defined in section 10 for byte/count admission, including still-charged expired/deleting payloads. The DDL bounds cached values; the typed methods must maintain them. No admission query depends on a nonexistent kind-leading index or scans all document objects.

Cancellation is actor- and document-scoped. Resolve/check an existing committed target first: return already-committed and never claim rollback. Otherwise create a committed cancellation fence naming the target request key, including when the target preparation has not arrived. Target admission and final commit check fences in the same immediate transaction. Retain a fence through the target's admissible retry interval and for as long as target prepared work exists. Historical target IDs deliberately are not FKs to expiring receipts.

**12. Retry, expiry, and replay semantics**

New externally supplied idempotency keys are `v2.<issued_at_ms>.<32 lowercase hex random>`. The whole fixed key, canonical request digest, document scope, and server-derived actor key define a replay. The key's timestamp is an age bound, not authentication. Canonical digest includes the operation kind, all effect-bearing request fields, and expected source/publication identity.

For a new request with no receipt, require issued time no more than 60 seconds in the future and no more than 15 minutes old. Persist the fixed work deadline; a retry cannot move it forward. Retain ordinary terminal receipts until seven days after key issue, at least through work settlement. Existing authenticated matching receipts may be returned after the 15-minute new-work window. Once a receipt has expired or been pruned, an old key returns `410 operation_expired`; it never creates a new effect. Changing timestamp/randomness produces a new request, which must still pass normal expected-head and authority checks.

Internal maintenance operations use a server-only key namespace and are not accepted by public routes. Account/document erasure may outlive the external admission window; operation existence and explicit recovery ownership permit its continuation. It is not re-admitted from a forgotten old client key.

A conflicting digest/kind for an existing key returns conflict. Receipt reads must recheck access before returning private result contents. Expiring a receipt does not remove an accepted suggestion's state or an activated publication's identity. Prepared work is never purged by a terminal-receipt cleanup query.

Replay uses three partial unique indexes, each matching its natural lookup predicate:

| Scope | Lookup predicate | Index |
|---|---|---|
| Document | `document_id=? AND actor_key=? AND request_key=?` | `operations_request_document` |
| Account | `account_id=? AND actor_key=? AND request_key=?` | `operations_request_account` |
| Server | `document_id IS NULL AND account_id IS NULL AND actor_key=? AND request_key=?` | `operations_request_server` |

Use dedicated typed lookup methods, not a generic OR predicate across all scopes. No COALESCE expression matching or fake non-null scope identity is needed. Validate all three query plans and same-key uniqueness boundaries.

Every terminal operation row requires non-null `receipt_expires_at` at or after `completed_at`, enforced by SQL. When recovery settles work after its nominal receipt horizon, choose at least the completion time; it may then be immediately due for safe cleanup. A prepared row requires a persisted `work_expires_at` unless its kind is internal resumable work: `erase_account`, `erase_document`, `rotate_links`, `backup`, or `journal_compact`. Those five may have no abandonment deadline, with liveness monitored through progress time and writer ownership. Ordinary user mutations, staging, execution, and journal appends cannot omit a work deadline. Expiry blocks new activation/renewal as specified; it never authorizes dropping an unsettled physical allocation.

Do not retain one large seven-day receipt for every internal journal flush. Set terminal expiry by kind: externally retried mutations/cancellations and administrator backup/rotation results use the seven-day rule; internal journal append/compaction receipts can expire 60 seconds after settlement; stage and execution rows can expire one hour after terminal settlement once no writer/lease/fence still depends on them. A final erasure removes its scoped internal row as described above. Clear completed internal plans immediately. Collaboration update retry IDs remain part of the checked journal framing and CRDT updates remain idempotent; short internal receipt retention does not redefine a WebSocket retry as a new non-idempotent text command.

Enforce finite operation row admission: start with 8,192 retained rows/document and 131,072/deployment, and at most 128 prepared operations/deployment in addition to the tighter per-kind limits. Reserve bounded slots for erasure, cancellation, and recovery so ordinary work cannot fill all capacity and prevent cleanup. Tune limits only with recorded measurements. Backpressure must happen before admitting more source bytes, and worker cleanup must run periodically even on an idle server. No terminal kind may default to an infinite NULL expiry by accident.

Expiry worker: select at most 128 eligible rows in `(receipt_expires_at,id)` order, check absence of unresolved allocations/leases and still-needed cancellation/execution dependencies, then delete. Compact large terminal plans to `{version:1}` when their recovery role ends; retain the minimal response envelope. Repeated worker runs are idempotent.

**13. Object publication transaction protocol**

All object-producing operations follow the same phases, with a kind-specific final state update:

1. Parse and validate the request, paths, representation limits, actor, expected head, and idempotency window. Compute exact encoded content/digests where feasible in bounded staging memory or a guarded temporary file.
2. In `BEGIN IMMEDIATE`, recheck document/account lifecycle and authority. Resolve matching receipts/cancellation fences. Allocate operation and new immutable object rows, add reservations/counters, and acquire write/stage leases. For reused objects, atomically require available state and acquire protection. Commit admission.
3. Write through `FsStore` outside SQL. A task owns its reservation and I/O guard until the blocking filesystem task actually finishes. Cancellation of the HTTP future does not imply cancellation of `spawn_blocking` work. Verify length/digest and required fsync/rename durability before settlement.
4. In an immediate transaction, verify operation/generation and settle each successful physical allocation from reserved to measured bytes. Set measured length, zero its reservation, and clear `allocation_operation_id` together. It becomes available but remains protected by operation leases, which retain the staging relationship until activation/abort. Uncertain allocations remain charged and associated with the allocation operation.
5. At final activation, recheck current authority, cancellation, lease validity, and expected source/publication generation. Establish complete roots/checkpoint closure and update visible pointers, counters if still needed, annotation effects, and committed receipt atomically. Increment `catalog_revision`. Only now acknowledge a durable effect.
6. Release operation leases after durable roots exist. Set GC deadlines for superseded/unreferenced objects. Available objects already have no allocation-operation reference, so releasing the remaining leases permits terminal receipt cleanup. If activation loses its CAS, abort the effect and schedule the new unreferenced allocations for safe reclamation; do not overwrite the winning head.

Checkpoint/closure insertion and source activation may share one transaction. The implementation may combine phases 4 and 5 when all bytes have settled, but may not expose an incomplete object graph. A result is never acknowledged between object PUT and SQL commit.

Every public object-writing path must use this protocol: room persistence, source upload, checkpoint creation, suggestion acceptance, source assets, rendered publication, agent staging, and journal compaction. A catalog-free Store fallback and direct mutable-key write paths are removed from production.

**14. Per-document journal and compaction**

Retain current bounded journal record framing initially, including retry IDs, fragment checksums, complete record checksums, and record count/size limits. A new segment may contain records from only one document. When a large logical update spans physical segments, the operation commits their complete set and sequence advance atomically; recovery must never replay half an update.

`objects` rows of kind journal_segment/journal_base have typed epoch and first/last sequence columns for indexed recovery. Segment descriptors are physical objects, not another metadata table. The document holds its active base pointer and the current durable sequence. Range columns summarize the actual checked frame contents; validator/recovery verifies them. No deployment-wide segment sequence, coverage table, shared-segment rewrite, manifest-shard table, or remote journal-head election remains.

Append reads the document's next sequence and generation, prepares one bounded source writer, writes the segment set, and atomically marks its complete object set live while advancing durable sequence/source generation. Any referenced asset/recipe dependencies become live roots in that transaction. Increment the singleton catalog revision for backup diagnostics, but do not use it as a global compare-and-swap that makes independent documents conflict.

Compaction captures state at durable sequence S under the per-document writer gate, writes a new immutable base, and then compares the captured generation. On success it advances epoch, installs the base, marks its complete source dependencies live, and retires old base/segments through S. The simplest first implementation blocks new durable appends for that document during this bounded operation; other documents continue. Buffered edits remain bounded and unacknowledged until appended. Do not implement tail rebasing during this migration.

A checkpoint may describe a source tree but not all CRDT/room state necessary for replay. Never delete journal segments merely because `MAX(checkpoint.sequence)` is newer. Only an independently verified journal base covering the full state allows old journal coverage to retire.

After compaction, a root reconciliation clears obsolete live-root flags for dependencies no longer needed by the new base. Perform this from the captured complete set under the same document gate and generation check. If the set exceeds a single bounded update page, conservatively leave surplus live roots, persist a small cursor in the compaction operation, and clear them in pages while fencing competing root changes; do not acknowledge completion until reconciliation is safe. Temporary extra retention is acceptable; premature reclamation is not.

Recovery opens the catalog after obtaining the exclusive deployment writer lock, establishes a fresh writer generation, settles prepared operations, then reconstructs each room from its active base and consecutive live segments. Validate document identity, epochs, sequence coverage, complete fragments, lengths, and digests. A missing acknowledged range is a recovery error. Never silently initialize an empty room on corruption.

Reuse the existing process-lifetime `fs2` lock at `DeploymentPaths.writer_lock`, currently `<deployment>/state/writer.lock` in `config.rs`. The server/CLI already use this deployment lock. The new converter and any administrative repair tools must use the same path and ownership protocol; that participation is a v2 requirement, not a claim that those tools already exist. Do not introduce a separate `catalog.lock`. A lock file's existence is not evidence of a live writer; the OS lock is. Take it before opening for mutation and hold it until all catalog/object tasks have joined. A generation string alone does not prevent a second process from writing files.

Recovery must make an explicit decision for each prepared kind:

| Prepared work | On ordinary restart |
|---|---|
| Source publish/checkpoint/journal append/agent source apply | Settle known files/reservations, then abort an uncommitted user effect by default. It was never acknowledged. Do not auto-activate using stale captured credentials |
| Display publication/agent staging | Preserve verified staged data within its original deadline if resumable by a newly authenticated request; adopt the new writer generation only after rechecking scope and authority. Otherwise abort and reclaim |
| Journal compaction | Resume the finite internal operation only after verifying captured source generation and complete new base; otherwise abort safely and retain old roots |
| Agent execution | Abort every old prepared epoch; issue a new one through the live runner connection |
| Account/document erasure | Resume its internal stage/cursor after physical I/O reconciliation |
| Link rotation | Resume against configured source/destination keys; do not infer a new primary from row order |
| Backup | Resume only an identified incomplete destination owned by that operation, or abort after its old copy tasks are known stopped |
| Annotation-only/cancellation/explicit checkpoint deletion | Their effect and state normally commit in one SQL transaction; a prepared row, if one exists, cannot be treated as proof the effect occurred |

On restore into a different root, always abort the backup operation copied inside the snapshot; never resume it against the original deployment's destination path. Resume only internal erasure/rotation/compaction work that passes restored-identity/key/state validation. Copied staged user work may be discarded, and old execution epochs always expire. The restore verifier distinguishes missing unfinished allocations from missing acknowledged roots.

Adopting any surviving internal operation updates its writer generation under the catalog lock. Late completion callbacks bearing the old generation cannot activate state. After ordinary process restart the OS lock establishes that old local tasks are gone; during an in-process cancellation, joining their guards is still required.

**15. Garbage collection and leases**

GC is an explicit bounded worker. It performs no graph traversal of source recipes because checkpoint closure has already been flattened. It uses actual root flags, indexed checkpoint references, leases, document pointers, and the global prepared-backup fence.

When a checkpoint is removed, collect its at-most-16,384 object IDs, delete the checkpoint/closure and decrement both reference counters in one transaction, and schedule those objects with `gc_after`. Clearing a live/publication root or releasing the final staging protection likewise schedules candidate work. Root additions clear/postpone a pending GC candidate under the same transaction.

Use distinct constants and clocks for the 15-minute display-preparation lifetime and 15-minute physical supersession grace. Current publication GC already applies `STAGING_TTL_SECS` to retirement-marker age in `server/publication.rs:695`; it is not used solely for staging. v2 preserves the publication grace duration, but starts it when transactional root removal schedules the candidate rather than when a later sweep first writes a retirement marker. Applying the same grace to other ordinary object supersessions is a new uniform v2 policy. A newly superseded candidate uses at least `now + grace`, even if an old stale candidate deadline was earlier. Root checks remain mandatory after the deadline. Full document erasure can bypass supersession grace after stopping writers/readers safely.

Each pass examines at most 256 candidate objects using `(gc_after,document_id,id)`. For each candidate in an immediate transaction:

- Require available state, due deadline, no live/publication root, no checkpoint reference, no active lease, no current document head/base/publication pointer, and no prepared backup freeze.
- If a durable reference exists, clear `gc_after`; removing that reference later must schedule it again. If only a lease blocks removal, postpone to its deadline rather than repeatedly examining it every tick.
- Set state to deleting and retry deadline atomically. From then on all new reference/lease acquisition fails. Do not physically delete inside the SQL transaction.

Delete at most 64 keys per physical batch using `delete_each`. Only Deleted/Absent confirms release. Failed/Uncertain keeps the row, full charge, and a retry deadline. After confirmation, delete its SQL row and decrement the three levels of counters in one transaction. A second confirmation is harmless. A deleting object can never be resurrected; a writer needing those bytes allocates a new physical ID/key.

Read leases default to 120 seconds and heartbeat every 30 seconds; configurable values must preserve that margin. A caller unable to renew stops serving/using the object before it loses protection. Write/stage leases cannot extend beyond the owning operation's supported lifetime. Renewal after expiry is rejected. Lease generation is checked on mutation paths. Expiry cleanup pages at most 256 rows and schedules affected objects.

Lease acquisition is a typed immediate transaction, not an unguarded INSERT. For reads/reuse, insert conditionally from the matching available object and require one affected row; for a new write, require the allocated object to name this same prepared allocation operation. Recheck operation scope/generation/deadline and document authorization as applicable. GC's claim and these state checks run under the same SQLite write serialization, so either the lease precedes the claim or acquisition fails. No async I/O may intervene between checking state and inserting protection. The FK alone establishes object existence, not availability; direct SQL lease inserts are intentionally not the product API.

The absence of a lease after its deadline does not prove an allocated PUT has stopped. For local `FsStore`, wait for the actual guarded blocking task, or after restart rely on the exclusive process lock plus reconciliation of uniquely named temporary/final files. A physical allocation with an uncertain write outcome stays reserved and its operation is not purged until settled. For a future remote backend, a negative HEAD alone is insufficient if a timed-out PUT could still finish later; such an adapter must provide a settlement strategy or retain/quarantine the allocation. The migration may not hide this race behind a TTL.

Checkpoint deletion and document deletion are intentionally distinct. Deleting a document first sets deleting state, withdraws access, aborts/revokes admissible new work, stops room writers, waits for in-flight I/O settlement, clears publication/live pointers and roots, removes annotation protection/checkpoints in pages, and queues all its objects. The document/owner counters and slug reservation remain until every object/allocation is physically settled. The final transaction removes scoped operations/leases and remaining relational children, decrements document counts, then deletes the document. Restrictive object/checkpoint/operation FKs make premature teardown fail.

**16. Retention without candidate/job tables**

Default balanced retention keeps the current checkpoint, all labeled checkpoints, and checkpoints protected by unresolved source annotations. Among unprotected/unlabeled routine checkpoints, keep at most the newest 50 and at most 30 days of age. The conjunction is deliberate: a routine point is a candidate when it exceeds either bound. A fresh document with only its current checkpoint retains it regardless of age.

Manual mode disables automatic history deletion. Custom settings are limited to `routine_count` and `max_age_ms` in the document retention payload, each nullable to disable that bound. Account preferences provide defaults for documents without overrides. Remove tier arrays, account-wide graph-budget eviction, pressure-specific ordering, exact persistent previews, and automatic override of protected milestones.

There is an independent maximum of 4,096 checkpoints/document and 1,000 labeled/protected checkpoints/document, subject also to the cumulative reference ceilings in section 9. These maxima cannot all be reached simultaneously for large closures. When a new protected checkpoint would violate a bound, reject the change with a clear limit error; do not evict a protected point. The importer detects violations before conversion and requires explicit remediation. A configured limit can be raised only within the stated implementation ceiling; exceeding that ceiling requires a reviewed schema/spec change and validation, never silent loss or an importer bypass.

A worker selects due active documents using `retention_due_at`, loads at most that bounded document's checkpoint metadata and protection pointers, and calculates the current policy result. Newly eligible checkpoints receive `eligible_after = now + 24 hours`. No separate candidate list is persisted. Points that become protected or fall inside the policy lose eligibility. A policy revision restarts grace for newly eligible points and clears obsolete eligibility; it cannot retroactively skip grace. Track the last evaluated account/document policy revisions in the small retention payload so restart can detect a changed policy.

At deletion, recheck current ownership, lifecycle, policy revisions, current checkpoint, label, annotation protection, and due grace in the same transaction. Delete at most 32 checkpoints/pass and at most 32,768 closure rows in one transaction, whichever stops the batch first, and schedule their physical objects. Capture exact edge counts before deletion and decrement document/server reference counters atomically; retries do not decrement again. A maximum-sized checkpoint fits within this work budget. These are initial row-work bounds, not a latency guarantee; lower them if measured transaction time exceeds the service budget.

Manual deletion accepts explicit checkpoint IDs, at most 128, with normal authority/protection checks and an idempotent operation. Apply the same 32,768-edge transaction budget: reject a larger request with `checkpoint_delete_batch_limit` and the supported budget before removing anything. The client may submit smaller explicit requests, each with its own idempotency key. Keep each accepted request atomic rather than introducing another resumable deletion-plan cursor. A manual preview reports candidate logical metadata and an estimated reclaimable object union. It labels the estimate; intervening references and reader leases can change actual reclamation. The UI reports actual deletion/reclamation progress from subsequent state rather than a durable immutable preview plan.

**17. Account erasure, rate state, and cost state**

`erase_account` changes the account to erasing and rotates its session generation in the first transaction. New mutation attribution to that account is thereafter refused or scrubbed at commit. Its typed plan contains only the current stage and stable cursor. Existing stages become: owned document teardown, grants/bookmarks, annotations/replies, checkpoint attribution, account-scoped operations, final verification.

Use author-leading indexes to process at most 250 attribution rows per transaction. Delete authored annotations/replies according to current behavior; clear account ID and replace display attribution with `Deleted user` on retained checkpoints. Scrub private attribution inside still-retained operation result/plan payloads according to their known types, or expire them after their effects settle. Never scan arbitrary JSON for accidental string matches. Immutable object formats should avoid embedding account identifiers; handle already embedded v1 attribution explicitly during conversion.

At final account deletion require zero owned documents/charges, no remaining restrictive attribution FKs, and no prepared scoped work. Delete its own internal erasure row and terminal scoped rows as part of finalization. Other users' stale bookmarks are non-authoritative and may be lazily pruned; they contain document/link identifiers, not erased profile data.

Replace upload/checkpoint rate tables with a bounded token-bucket component keyed by stable owner ID and action. Preserve current configured rates where possible. State resets on process restart; persistent object quotas still prevent storage bypass. Bound entries and expiration so anonymous traffic cannot grow the map indefinitely. Do not repeatedly serialize all buckets into server JSON as a disguised bucket table.

`server_state.cost_json` preserves the existing versioned durable aggregate transfer-meter snapshot. It is a bounded aggregate blob, not an event log. Keep fail-closed budget recovery if decoding fails. Writes update only the cost column and revision/time through the catalog API, never replace the entire singleton row from a stale in-memory copy. The same rule applies to keyring and maintenance fields.

**18. Backup, restore, and recovery ownership**

The SQLite catalog is the sole authoritative metadata index. Object-store listings cannot reconstruct account permissions, annotations, or committed heads. Do not advertise object-only recovery after deleting journal manifests from the architecture.

For an online consistent backup, prepare one server-scoped backup operation that freezes physical GC and compaction retirement. Finish any physical deletes already in flight before taking the SQL snapshot. Briefly gate activation while creating the SQLite snapshot and capture its revision; normal new allocations/appends may then continue. Immutability and the GC freeze ensure the snapshot's available objects remain readable. Copy exactly the available objects listed in the snapshot plus required deployment identity/secrets, verifying each digest. Prepared allocations in the snapshot are recovery work, not acknowledged roots; the restore path aborts/reconciles them instead of treating missing unfinished payloads as missing committed data.

Write the completion manifest last. It identifies catalog schema/format version, deployment ID, snapshot revision, object entries, and secret-key versions. Only then commit the backup operation and release the GC freeze. An interrupted backup remains incomplete; recovery either resumes it from its typed cursor/manifest or aborts it and cleans its private destination. A time deadline alone must not release the freeze while the copy task is still using source files. Backup destination allocation/free-space limits are separate from user object quota.

Restore into a new root, acquire exclusive ownership, verify every manifest entry, run integrity and foreign-key checks, and audit root closure/counters before marking it usable. Bump writer generation and reconcile prepared operations before serving traffic. Old execution leases are invalidated. A retained snapshot may contain old available objects no longer current; that is safe extra retention, not evidence to drop a current root.

Use a new backup format number for the changed catalog/object envelopes. Do not teach the normal v2 restore path to guess between incompatible v1/v2 schemas. v1 backups can be restored by the v1 tool and then passed through the offline converter.

**19. Complete old-to-new mapping**

This inventory includes all 45 baseline tables and the two persistent runtime tables. “Remove” means remove the old runtime SQL and its compatibility model, not merely stop creating the old table.

| v1 table | v2 destination and conversion rule |
|---|---|
| `accounts` | `accounts`; map profile/provider identity, timestamps, status, quota preferences; rotate sessions at cutover |
| `documents` | `documents`; storage ID becomes PK; map ownership, title key, source/publication state; counters rebuilt from objects |
| `grants` | `grants`; collapse each account/document to strongest role |
| `links` | `links`; assign stable IDs; preserve valid credential bytes/hash through verified resealing |
| `guests` | Account bookmark items with matching link ID/generation; omit revoked/nonexistent links |
| `totals` | Remove; recompute document/account/server counters from target objects |
| `checkpoints` | `checkpoints`; preserve event ID/order/content; build a complete physical closure |
| `comments` | `annotations`; explicit kind, selector/context envelope, suggestion state, resolution/protection |
| `replies` | `replies`; map author identity and parent annotation |
| `pending_deletes` | Set matching target object to deleting only after final target liveness analysis; never import a deletion instruction against a retained root |
| `catalog_operations` | Settle v1 preparations before import; old external receipts expire at cutover; do not import arbitrary legacy intent blobs |
| `journal_state` | New server generation/revision and reconstructed per-document base/sequence; no old deployment journal CAS |
| `journal_preparations` | Settle with v1 recovery; remove resolved plans; uncertain acknowledged effects block conversion |
| `journal_segments` | Replay v1 data into a verified v2 base for each document, or explicitly re-encode per-document segments; no copied shared segment ownership |
| `journal_retirements` | Reconcile source before import; source root is preserved for rollback, not physically cleaned by the converter |
| `account_activity` | Account `last_active_at` |
| `erasure_batches` | Finish v1 erasure before normal import; an explicitly supported resume adapter may instead translate the finite stage/cursor after validation |
| `maintenance_jobs` | Settle or abandon source-only jobs after source quiescence; do not copy accounting reservations as historical charges |
| `checkpoint_budgets` | Remove; new in-memory checkpoint rate buckets |
| `journal_segment_coverage` | Used only by v1 replay to reconstruct each document; removed in target |
| `journal_bases` | Replay source bases into new per-document v2 bases |
| `journal_manifest_shards` | Input to v1 recovery only; removed |
| `link_keyring` | Verified bounded key descriptors and one explicit active key in server singleton |
| `link_key_rotations` | Finish/resolve before conversion; fresh v2 rotations use one typed operation |
| `object_accounting` | Input evidence for source verification; target object inventory is rebuilt from verified retained bytes |
| `object_reservations` | Source work must settle; new converter allocations receive new target reservations |
| `deletion_discovery` | Finish source document teardown or explicitly exclude it; target deletion uses complete object inventory |
| `upload_buckets` | Remove; new in-memory upload buckets |
| `journal_readers` | Source readers stop under exclusive conversion lock; no live lease imported |
| `document_results_metadata` | Remove; derive from canonical source format |
| `account_examples` | Account onboarding slots; preserve completed/deleted semantics |
| `agent_objects` | Expire at cutover; new staging uses protected agent_payload objects |
| `agent_cancellations` | Old v1 request namespace is invalid after cutover; do not carry fences into a new admissible request namespace |
| `agent_execution_leases` | Invalidate at cutover; new execution operations must be issued |
| `account_quota_preferences` | Account preference payload/revision; map supported simple policy, report discarded policy features |
| `source_history_encodings` | Decode/verify recipes; write v2 immutable recipe locators; remove SQL mapping table |
| `source_history_objects` | Verified physical chunk input; target objects plus flattened checkpoint references |
| `source_history_checkpoint_files` | Input to validation; recompute complete target closure from actual manifests/bytes |
| `source_history_write_leases` | Source writers/readers must stop and settle; target converter creates fresh temporary protection |
| `quota_retention_jobs` | Remove terminal/planned work; v2 recalculates eligibility and starts fresh grace |
| `quota_retention_candidates` | Remove; no candidate import |
| `checkpoint_retention` | Preserve original event ancestry/protection semantics; clear grace and recalculate with a fresh window |
| `document_retention_policy` | `documents.retention_mode`, revision, simple policy payload |
| `checkpoint_asset_refs` | Verify against actual checkpoint tree; flatten into checkpoint_objects |
| `checkpoint_asset_sets` | Remove; closure completeness is mandatory |
| Runtime `source_history_gc_encoding_state` | Remove; target object-row GC deadlines are the work queue |
| Runtime `cost_state` | Validated server singleton cost payload; preserve remaining transfer budget conservatively |

The old TEMP room reservation table/view disappears with the old connection. Process-local coordinator reservations start empty only after pending source work has been drained or explicitly abandoned before cutover.

**20. Migration strategy and tooling**

Use schema version 2 even though software is unreleased. Keep the historical v1 SQL only as converter/test input outside the production migration list. A fresh v2 catalog is created from the new baseline in one transaction and initialized with deployment-specific singleton identity/key metadata. Set `PRAGMA user_version=2` in that same transaction. Opening a v1 catalog with the v2 server fails with an actionable conversion/new-root message; do not silently reinterpret version 1 or auto-delete it. Opening a newer version also fails.

At implementation time install the reviewed DDL as `crates/librepaper/migrations/0002_catalog.sql` and use version 2 as the fresh-install baseline. The startup version-1 rejection must happen before the ordinary migration loop; simply appending this CREATE TABLE script to the old loop would collide with existing tables. Versions after 2 may use normal incremental migrations. The spec artifact itself must not be loaded opportunistically from a working directory at runtime.

The standalone DDL intentionally does not set `user_version` or insert placeholder deployment secrets/identity. Loading it alone is not initialization and leaves `user_version` unchanged. The initializer must begin one transaction, execute the DDL, insert the fully specified `server_state` singleton, set `user_version=2`, and commit. Any failure rolls back all of those steps. The validator exercises this contract; a catalog reporting version 2 but missing its singleton must be rejected on opening, not silently initialized as another deployment.

The default development workflow may create a fresh root and reseed examples. No persistent user data is deleted automatically. For development data worth retaining, implement a separate one-shot tool under `tools/catalog-v1-import/`, outside the normal server startup path. It has `--source-data`, `--target-data`, `--dry-run`, and `--resume` arguments. A required explicit active link-key selection is accepted only when source key metadata is ambiguous; the tool must never pick a key by lexicographic/timestamp luck.

The converter operates offline and never mutates the source. Source must have been cleanly stopped with v1 recovery/erasure finished. It acquires the source deployment writer lock as well as an exclusive target lock. It rejects overlapping source/target paths, nonempty unrecognized targets, symlink escapes, source versions other than its explicitly supported v1 shape, or target identity inconsistent with a resume manifest. The old schema version alone is insufficient to identify evolving unreleased v1 files: inspect required columns and record a source schema fingerprint.

Conversion phases:

1. **Inspect.** Read-only schema/integrity/FK validation; enumerate owners/documents/checkpoints/current publications; verify available source keyring. Report invalid timestamps, unknown formats, title conflicts, missing roots, unsupported selectors, unresolved work, and all proposed policy/session changes. A dry run writes no target and does not claim full byte verification unless it actually reads the bytes.
2. **Freeze and inventory.** With source offline and locked, snapshot/catalog-hash the source and produce an external conversion manifest. Record source identity, schema fingerprint, converter version, target identity, per-document cursor, planned byte demand, and errors. Do not store migration progress as a thirteenth target table.
3. **Initialize target.** Create the exact v2 schema, initialize singleton/key metadata and owner rows, then creating documents with NULL heads. Import static sharing/profile data in bounded transactions. Use fresh session generations; force clients/runners to reconnect and reauthenticate.
4. **Recover live documents.** Run the read-only v1 journal/base decoder to reconstruct acknowledged source state. Verify sequence coverage and fragment checksums. Write a self-contained v2 per-document base and dependency closure. Do not call the v1 deletion workers on the source. A missing committed range blocks that document and completion of the default all-data conversion.
5. **Convert retained checkpoints.** Read actual trees, source recipes/chunks, and assets; verify logical content; assign new physical object IDs; rewrite locator envelopes while preserving user file bytes and event order. Compute target logical digests under the explicit v2 logical encoding. Maintain an external old-identity-to-new-identity map for conversion, translating live annotation protection as needed. Preserve original historical revision text when it is merely provenance. Never assume that a source ledger row proves an object's existence or length.
6. **Convert current publications.** Preserve rendered HTML/assets byte-for-byte except for necessary format envelope changes; retain public publication event IDs where possible. Establish the complete publication-root set. Old superseded rendered bundles need not be imported unless required by an explicitly supported retained root. The source copy retains them for rollback.
7. **Convert annotations and settings.** Map selectors, attributions, suggestions, original parent identities, bookmarks, and onboarding. A resolved suggestion whose acceptance receipt is intentionally not imported retains its accepted state and historical operation ID. Recalculate retention eligibility from the new simple policy with fresh grace. Unknown unsupported policy fields are reported and preserved in the conversion report; they do not silently influence v2 execution.
8. **Reconcile and verify.** Every target object has measured bytes; no unresolved converter allocation remains. Recompute counters and document counts. Check FKs, integrity, head pointers, exact checkpoint closures, live/publication dependencies, readability, and all acceptance inventory comparisons. No object in deleting state may be a retained root.
9. **Complete.** Write and fsync a conversion completion manifest containing source/target identities, object counts/bytes, document/checkpoint/annotation counts, exclusions if explicitly requested, and verification results. Only a complete conversion may be selected as the new deployment root.

Resume uses deterministic mappings recorded in the external manifest and checksummed per-document progress files. Never assume an operation succeeded merely because a progress cursor advanced. Verify target SQL commits and physical digests before skipping a completed item. Converter objects use unique keys and the target allocation protocol; repeated conversion must neither duplicate logical checkpoints nor double-count storage. Source data remains the authority until final completion.

A partial import is not the default. An explicit document allowlist may produce a deliberately partial new deployment, but the completion report must list excluded documents and dependencies. A normal failed conversion leaves the source usable by its old binary and the target marked incomplete.

Cutover stops v1, verifies target completion, selects the new data root/config, starts v2, and checks health/sample reads. Keep the original binary/root and a verified backup until acceptance is complete. Before any v2 mutations, rollback is selecting the old root. After v2 mutations, selecting the old root loses those new effects; there is no automatic reverse migration. Export/reconcile new content explicitly if rollback after writes is required. Breaking schema compatibility does not authorize silently discarding new data.

**21. Required code changes and deletion targets**

| Area | Required work |
|---|---|
| `storage/catalog/mod.rs` | Fresh v2 DDL/version guard; typed row IDs/times; one mutation boundary; no public arbitrary production connection writes |
| `storage/catalog/accounts.rs` | Counter-based quota admission; simplified preferences/ownership/erasure; delete union/dedup/path-classification and serialized-field billing evaluators |
| `storage/catalog/documents.rs` | Internal ID queries, title key, explicit heads, source-format derivation, removal of pending-publication/derived metadata paths |
| `storage/catalog/access.rs` | Single effective grants, stable link IDs/generation, bookmarks, singleton key metadata, typed rotation operation |
| `storage/catalog/checkpoints.rs` | Event identities, nonreused sequence allocation, complete closure commits, simple protected retention |
| `storage/catalog/comments.rs`, `agent_annotations.rs` | Typed selector/context envelopes, author FKs, suggestion/protection transaction contracts |
| `storage/catalog/operations.rs` | Canonical allocation/settlement/activation/deletion APIs and finite operation enum; bounded receipt cleanup |
| `storage/catalog/agent_source.rs` | Route agent source effects through the same allocation/activation boundary; no separate receipt, accounting or source-authority path |
| `storage/catalog/journal.rs` | Replace deployment-journal SQL entry points with document-scoped operation/object/head methods; remove old journal table queries |
| `storage/catalog/source_history.rs`, `asset_history.rs` | Remove old SQL graph and copy-of-size model; retain codec/conversion helpers and closure construction under an appropriate name |
| `storage/catalog/retention.rs`, `pressure.rs` | Replace persistent job/candidate/pressure eviction code; keep bounded per-document policy and metadata-pressure admission |
| `storage/catalog/agent_objects.rs`, `agent_cancel.rs`, `agent_lease.rs` | Object-backed staging, single operation receipts/cancellation, execution epoch rows |
| `storage/catalog/room_edits.rs`, `execution.rs` | Shared in-memory reservation/admission boundary, cancellation-safe lifetime, no stale owner totals |
| `storage/journal/{store,coordinator,runtime,recovery,segment}.rs` | Per-document segments/bases, SQL-authoritative heads, complete fragment recovery, no shared coverage/manifests |
| `storage/maintenance.rs` | Object-row GC, explicit erasure, bounded retry/lease/receipt cleanup; remove prefix discovery from normal deletion |
| `storage/blob.rs` | Immutable registered-key writes and tracked completion; preserve filesystem fsync/free-space guards; no ordinary mutable application object paths |
| `storage/backup.rs` | v2 snapshot/manifest protocol, GC freeze, FK/root/counter checks, new format number |
| `document/{store,history,quota,session}.rs` and `room/*` | Remove catalog-optional/legacy persistence model; adapt revision/protection/root semantics; durable acknowledgement only after catalog commit |
| `server/{publication,publication_http,quota,sharing,onboarding,cost,serve}.rs` | Revised publication/retry API, quota display, bookmarks, singleton cost, bounded workers |
| `cli/*`, server MCP, web callers | v2 request IDs, changed quota/retention responses, source busy handling, selector/revision adapters |
| `seed/mod.rs`, fixtures, browser test setup | Create only v2 data; remove old raw SQL/JSON persistence assumptions |
| `tools/catalog-v1-import/` | Isolated read-only v1 decoders and offline converter; no v1 production write path |
| Protocol/manual/cost/scaling documents | Rewrite behavior claims and regenerate changed measurements |

It is acceptable to retain module filenames temporarily while changing internals. It is not acceptable to leave unused old SQL methods, compatibility fallback branches, old object-path writes, or runtime DDL “for later.” At completion, search production source for every removed table name and account for any remaining occurrence. Legacy names should exist only in historical review/spec text, converter code, and converter fixtures.

**22. Delivery phases and gates**

Implement in dependency order. Intermediate commits may use feature-gated v2 test fixtures, but the completed product shall not have dual writers or two active accounting models.

| Phase | Deliverable | Gate before next phase |
|---|---|---|
| A | DDL, ID/time/payload types, schema initialization, direct constraint tests | Exactly twelve persistent tables, zero triggers; invalid-row/FK/uniqueness tests pass |
| B | Canonical objects, reservations, counters, writer task lifetime, lease/GC state transitions | Quota/replacement/cancellation/delete race tests and counter recomputation agree |
| C | Document/checkpoint manifests, flattened closures, source and publication activation | All five formats round-trip, sharing and suggestion atomicity work |
| D | Per-document journal/base recovery and compaction | Crash matrix proves no acknowledged update loss; shared-segment code gone |
| E | Simplified retention, accounts/erasure, operations/agent staging/retry/key rotation | Policy/race/expiry/security contracts pass; no durable candidate/rate tables |
| F | Backup/restore and converter | Complete source-to-target fixture conversion, interrupted resume, rollback rehearsal |
| G | CLI/web/protocol update, delete v1 runtime paths, performance and size measurements | No stale schema consumers; full test suite and targeted end-to-end flows pass |

Do not run a user data conversion or cutover merely because the spec or fresh-schema tests pass. Those are separate implementation/deployment actions. This specification authorizes no actual deletion of existing roots.

**23. Acceptance tests**

Tests must assert behavior and invariants, not simply reproduce SQL strings. Existing useful regression scenarios should be ported rather than dropped because a table disappeared.

| Group | Mandatory cases |
|---|---|
| Schema | Exact twelve-table inventory after startup, meter creation, worker runs and backup; zero triggers; no runtime DDL; strict numeric types; non-null PKs; complete kind/scope matrix; conditional prepared deadlines and mandatory terminal deadlines; initializer version/singleton atomicity |
| Identity | Same event/object ID in two documents cannot create cross-document references; slug rename leaves objects/history intact; sequence allocation does not reuse deleted values |
| Accounts | Anonymous owner quota is enforced without login authority; system owner ID cannot authorize public callers; title collisions on transfer detected |
| Grants/links | Highest legacy role mapping; revoke effective grant; expired/rotated token fails; bookmark does not authorize; no-links key rotation and interrupted reseal recover |
| Object bytes | Wrong digest/length, oversized encoding and allocation overflow fail; no unregistered write; same digest in distinct physical versions charged separately |
| Accounting | Allocation adds reservation; settlement swaps to measured charge and clears allocation owner while leases remain; replacement charges old plus new; duplicate abort/delete confirmation does not refund twice; failed delete remains charged; zero-byte versus unknown allocation distinguished; stage subset counters include deleting objects; transfer with in-flight allocation and live edit moves counters correctly |
| Closure | Shared chunk across checkpoints retained until last reference; empty asset set valid; typed commit rejects incomplete/allocated/deleting dependencies; tree included in closure; per-checkpoint/document/deployment edge limits and concurrent final admission; exact counter decrement on deletion/rollback; identical checkpoints still consume repeated edges |
| GC races | Read versus GC claim; writer reuse versus claim; checkpoint creation versus retention; expired stage with still-running PUT; cancel of spawn_blocking caller; delete success before SQL acknowledgement; stale worker generation |
| Journals | Per-document segment invariant; complete multi-segment fragment commit; no half-update replay; no sequence gap tolerated; crash before/after SQL ack; compaction preserves CRDT state independently of checkpoint tree; unrelated document remains writable |
| Publications | Source save does not publish; two staged display preparations race on expected current ID; old bundle readable during preparation; complete root switch; lost activation response replays original identity/attribution; superseded bytes remain charged through grace |
| Suggestions | Concurrent accept/cancel; accepted receipt and source change atomic; erasure during queued acceptance; resolved protection released; reopening cannot reference pruned source silently |
| Agents | Actor scope checked for stage/replay/cancel; old execution epoch cannot commit after renewal/replacement/restart; payload and receipt caps; cancelled target arriving later cannot apply |
| Retry expiry | Natural queries select each of the three replay unique indexes; matching receipt replay; scoped uniqueness and cross-scope key reuse; conflicting digest rejected; old forgotten key returns 410; future key outside skew rejected; cleanup retains fences for prepared targets; NULL terminal deadline rejected; resumed work can settle after nominal horizon without an infinite receipt |
| Retention | Latest/labeled/unresolved references protected; deterministic routine count/age; grace survives restart; policy change restarts correct grace; advisory preview can differ safely; hard quota rejects rather than overriding protected points |
| Erasure | One prepared erasure per account/document even with different request keys; authorized second request reuses progress; cursor resume; reused display handle not treated as account identity; parent annotation thread behavior; late attribution cannot reappear; final deletion waits for physical charge and restrictive references |
| Backup | Writes during copy do not invalidate snapshot; in-flight deletes settle before snapshot; missing object fails completion; incomplete backup not advertised; restore invalidates old epochs and preserves acknowledged journal state |
| Conversion | All v1 tables accounted for; missing runtime tables handled; malformed v1 data reported; recipes/chunks revalidated; restore event distinct from tree digest; current publication and annotations preserved; resumable conversion idempotent; source root unchanged |

The current catalog test command is `cargo test --offline -p librepaper --lib storage::catalog::`; it passed 70 tests during the preceding review. That historical result is not validation of the v2 implementation. Once implemented, run the full applicable Rust suite plus focused publication, room/journal, agent, backup, erasure, quota, and browser/CLI tests. Run the project's configured quality checks and update fixtures that intentionally encode changed behavior.

**24. Performance and boundedness acceptance**

Create reproducible synthetic catalogs using the following matrix, with a physical/catalog budget sufficient for each admitted fixture. Record catalog size including secondary indexes/WAL, reference-row count, prepared/terminal operation count, transaction wall time, SQL VM work where available, and physical bytes/requests for publication/journal workloads. Record hardware and SQLite version. Do not replace measurements with assertions that fewer tables must be faster.

| Workload | Documents | Checkpoints/document | Edges/checkpoint | Total reference rows | Expected result |
|---|---:|---:|---:|---:|---|
| Small | 100 | 50 | 128 | 640,000 | Admitted within reference ceilings |
| Many histories | 1,000 | 50 | 128 | 6,400,000 | Admitted within reference ceilings |
| Many documents | 10,000 | 10 | 64 | 6,400,000 | Admitted within reference ceilings |
| Largest single closure/history combination | 1 | 64 | 16,384 | 1,048,576 | At document ceiling; adding another nonempty closure fails |
| Deployment ceiling | 8 | 64 | 16,384 | 8,388,608 | At global ceiling; one edge in another document fails |

For the first three workloads, use 0%, 90%, and 100% adjacent-checkpoint physical-object reuse within each document. Report the resulting distinct object inventory separately. Reference-row count is unchanged by this reuse factor. Include labeled/manual-retention fixtures where necessary so the retention worker does not delete the intended benchmark population. Test rejection and rollback above each ceiling rather than attempting the former 67-million-row per-document product of independent maxima. These ceilings are provisional engineering budgets, not a claim that their worst-case latency/storage is acceptable; the release gate requires measured approval or lower ceilings.

Required structural properties:

- One ordinary allocation consults indexed document/owner/server rows and bounded local reservations; it does not traverse checkpoints or enumerate owners. Stage-byte/count and checkpoint-reference admission likewise read cached typed columns, not aggregate scans.
- Erasure uses author-leading indexes and stable cursors; work is proportional to the selected account's matching rows, with documented FK cascade costs.
- GC selects due object rows through its partial index and checks indexed reverse references. It does not scan an entire document's source graph for each checkpoint deletion.
- Object closure insertion/deletion is proportional to the selected bounded closures, with a 32,768-edge deletion transaction budget. Cumulative document/deployment ceilings and counter equality are tested. More reference rows from flattening are measured and accepted explicitly.
- Replay reads use the corresponding document/account/server scope unique index with the exact natural predicates in section 12; the DDL validator checks these plans. Execution checks use the document/epoch index.
- Terminal operations and stage/reader leases converge to configured retention bounds after idle maintenance; continuous traffic is not required to trigger cleanup.
- No mutex/SQL transaction spans filesystem network-like I/O, encoding, hashing a large object, or copying backup payloads.

Compare per-document journaling against the current mixed-document batching using the same edit trace. Report increased object count and fsync/write overhead, along with reduced recovery/ownership complexity. Keep per-document batching/debounce and existing size limits; do not reintroduce shared segments merely to improve one benchmark without revisiting the design decision.

A final invariant audit independently reconstructs counters and every durable root closure from SQL/manifests after randomized write/retain/delete/restart sequences. It must find no missing referenced object, no negative/double charge, and no deleting object reachable from a current root. The object inventory and the actual filesystem may contain temporary guarded files during active writes, but completed idle reconciliation must account for or reclaim all private leftovers.

**25. Definition of done**

The migration is complete when the application creates and operates only the twelve-table schema; all required product paths use it; no old accounting/graph/journal compatibility system remains active; the offline converter and backup/restore have verified completion semantics; documentation reflects the deliberate behavior changes; and the acceptance/performance evidence is recorded.

The success criterion is fewer independent facts and fewer recovery algorithms. The table count is an enforceable boundary, but a large JSON payload or an overgeneralized operations dispatcher that recreates the old subsystems fails the design even if the database contains twelve tables.

**Specification validation completed**

The revised DDL was executed in an isolated database using SQLite 3.51.2: twelve tables, all STRICT, zero triggers, successful integrity/FK checks. The accompanying [validate-catalog-v2.py](validate-catalog-v2.py) passed **186 checks** on SQLite 3.53.1, including the complete kind/scope and deadline matrices, scoped erasure uniqueness, all three natural replay query plans, counter ceilings, settled-object receipt cleanup, title/slug release semantics, and initialization rollback.

The validator also confirms **four SQL-permitted boundary cases**: incomplete checkpoint closure, closure edges to allocated/deleting objects, and a lease on a deleting object. These require prevention through the typed transaction API; the probes deliberately do not label them as schema-enforced guarantees. Tests of cached counter ceilings likewise do not establish counter equality to underlying rows; that remains an implementation transaction/audit obligation.

Reproduce with `python3 docs/specs/validate-catalog-v2.py` on a suitable SQLite build, or `uv run --no-project --python 3.14 docs/specs/validate-catalog-v2.py`. The script opens only an in-memory database. These checks establish that the proposed SQL is executable and rejects the tested invalid states; they do not validate the unimplemented application protocols, converter, or performance claims.
