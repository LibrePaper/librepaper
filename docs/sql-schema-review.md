SQL schema review — 2026-09-12

**Assessment: simplify the sources of truth before release.** The request is to review the whole SQL model from first principles, including breaking changes. SQLite, explicit transactions, and an object store are a reasonable foundation. The difficulty is that successive features have introduced overlapping representations of identity, object lifetime, accounting, and recovery. Fewer independent facts would make future features substantially easier to implement correctly.

This is a review and proposed design, not an implemented migration. No application code or existing database was changed. The pre-existing README edit is unrelated.

**Scope and evidence.** Reviewed the complete baseline: 45 tables, 51 explicitly declared indexes, and five triggers; also the two additional persistent tables created at runtime, the temporary reservation table/view, and their catalog, publication, journal, maintenance, account-erasure, and test consumers. The catalog has about 17,000 lines including tests; this review traces the principal relationships and mutation paths rather than claiming exhaustive verification of every possible interleaving.

`cargo test --offline -p librepaper --lib storage::catalog::` passed all 70 tests. Isolated probes against the actual migration on SQLite 3.51.2 reproduced the constraint gaps below, with foreign keys enabled. The probes exercise SQL acceptance, not an exploit through a public endpoint. Query-plan findings are structural; no production-sized performance benchmark was run.

**1. High priority: make physical objects and reservations authoritative once.**

The clearest design problem is visible in [accounts.rs](/home/vincent/repos/librepaper/librepaper/crates/librepaper/src/storage/catalog/accounts.rs:123). Account usage unions six representations: `object_accounting`, `source_history_objects`, `source_history_write_leases`, `object_reservations`, `pending_deletes`, and `checkpoint_asset_refs`. Rust then deduplicates them, distinguishes planned from measured sizes, takes conservative maxima, and detects disagreements. Admission adds several kinds of headroom and falls back to another accounting model when verification is incomplete.

Moreover, [deployment_admission_bytes_on](/home/vincent/repos/librepaper/librepaper/crates/librepaper/src/storage/catalog/accounts.rs:521) enumerates owners and runs the owner evaluator for each. Reservation paths call this within the one catalog transaction. A small write can therefore depend on the retained object graph across the deployment. Existing limits bound workload size but do not make this cost proportional to the write.

Recommendation: one physical-object row owns the measured length, kind, version, and charge owner. References describe reachability; they do not copy measured sizes. Reservations own prospective allocation, separately from actual allocation. Pending deletion refers to an object that stays charged until deletion succeeds. Maintain admission counters transactionally from those records, and keep full recomputation as an audit/reconciliation operation.

For mutable keys, represent the old physical version and replacement allocation explicitly: replacing a 10 MB object with a 12 MB object may temporarily require both. A single `MAX(bytes)` is not a universal peak-storage model. Shared journal objects also need an explicit charge owner separate from the documents whose edits they contain.

`totals` is not inherently bad denormalization. It becomes useful when there is one derivation and one update contract. Document the accounting equation for committed objects, temporary allocation, process-local edits, and maintenance borrowing. Remove the legacy verified/unverified fallback once every newly created document has the complete model.

**2. High priority: use one internal document identity throughout the relational graph.**

[documents](/home/vincent/repos/librepaper/librepaper/crates/librepaper/migrations/0001_catalog.sql:14) has a slug primary key and a unique storage ID. Product tables reference the slug, while object and operation tables reference the storage ID. The two namespaces meet in triggers through repeated subqueries.

This is already preventing straightforward foreign keys. `source_history_checkpoint_files`, `checkpoint_asset_refs`, and `checkpoint_asset_sets` name a checkpoint by `(storage_id, checkpoint_sha)`, but checkpoints are keyed by `(slug, sha)`. SQL accepts an asset reference to a nonexistent checkpoint; `foreign_key_check` reports no violation because that relationship is not declared.

Use the existing immutable storage ID as `documents.id`, retain `slug UNIQUE` as its public address, and reference `document_id` everywhere. A separate integer key could work, but introducing a third document identity is unnecessary unless measurements justify it. Then declare real composite checkpoint foreign keys:

```sql
CREATE TABLE checkpoint_assets (
    document_id TEXT NOT NULL,
    checkpoint_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    PRIMARY KEY (document_id, checkpoint_id, object_id),
    FOREIGN KEY (document_id, checkpoint_id)
        REFERENCES checkpoints(document_id, id) ON DELETE CASCADE,
    FOREIGN KEY (object_id) REFERENCES objects(id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
```

This is illustrative, not a complete executable replacement schema. Object identity and cross-document ownership constraints must be chosen together. Do not blindly cascade the object itself when a checkpoint disappears.

Checkpoint event identity, tree content digest, Git commit, journal sequence, and publication identity must remain different concepts. Rename ambiguous `sha`, `tree_sha`, `parent`, `at`, `by`, and `why` to express which concept each stores. Two checkpoint events can describe the same tree. A historical parent may legitimately have been pruned, so it should not automatically receive a restrictive live-row FK. The same qualification applies to annotation provenance.

**3. High priority: move graph garbage collection out of per-checkpoint triggers.**

[source_history_checkpoint_removed](/home/vincent/repos/librepaper/librepaper/crates/librepaper/migrations/0001_catalog.sql:513) examines the document's source object graph and deletes unreferenced encodings after every checkpoint deletion. A batch of K checkpoint deletions can repeatedly examine the same large graph. A limit on deleted checkpoint rows does not bound the work inside these triggers.

[checkpoint_asset_removed](/home/vincent/repos/librepaper/librepaper/crates/librepaper/migrations/0001_catalog.sql:628) adds another deletion policy involving retained checkpoints, writer leases, journal coverage, and journal bases. The comments explain valid protections: a live journal may still need an asset absent from the remaining checkpoints, and queued objects must remain charged. Preserve those protections.

Recommendation: relational cascades remove reference edges; the mutation transaction also records bounded GC work. A worker evaluates a bounded page of candidate objects against all roots, atomically fences new references, queues physical deletion, and releases accounting only after successful reclamation. Active readers, pending writers, publication state, and journal state all count as roots. Removing these triggers without that protocol would be unsafe.

Prefer a small explicit root vocabulary and relational edge tables over a generic EAV graph. Source encodings still need their file-to-chunk edges. SQL need not duplicate chunk order if the immutable recipe remains authoritative for decoding.

**4. High priority: give durable operational history a bounded lifetime.**

[journal_preparations](/home/vincent/repos/librepaper/librepaper/crates/librepaper/migrations/0001_catalog.sql:179) stores up to 256 KiB of plan text per preparation. Normal journal code marks plans resolved but does not prune them; the repository's explicit deletion is in seed reset. Resolved plans can accumulate even after the corresponding data is compacted.

Other operational tables need the same lifecycle audit: ordinary `catalog_operations` receipts, completed maintenance jobs, completed/stale retention jobs and their candidates, and execution leases for old conversation IDs. Agent receipts have a seven-day cleanup path, but that is invoked from `put_agent_object`; there is no analogous general receipt-retention policy. Upload counters clean old buckets only for the owner currently uploading, leaving inactive-owner buckets behind.

Specify separately: recovery evidence needed until commit, replay receipts needed until retry expiry, and optional audit history. Compact or remove terminal rows in bounded maintenance batches with indexes on the actual expiry cursor. Never delete unresolved preparations, live leases, or cancellation evidence still fencing an admissible retry. Where old requests must remain recognizable forever, retain a compact tombstone rather than the whole plan/result.

**5. Medium priority: enforce fundamental invariants in SQL.**

The schema's checks are uneven. Actual probes accepted:

| Probe | Current result | Required invariant |
|---|---|---|
| `maintenance_jobs.id = NULL` | Insert succeeds | Every durable identity exists |
| `documents.counted_size = 'garbage'` | Update succeeds | Counters contain integers |
| `documents.example = 7` | Update succeeds | Boolean is 0 or 1 |
| Asset reference to missing checkpoint | Insert succeeds | Live reference points to a live checkpoint |
| One prepared and one running link rotation | Both insert | At most one active rotation |
| Two keyring rows with primary status | Both insert | One primary designation if status is authoritative |

Nine ordinary rowid tables have nullable text primary keys: journal preparations, segments, retirements, bases, manifest shards, readers, maintenance jobs, keyring, and key rotations. SQLite permits this unless the key is explicitly `NOT NULL` or the table is STRICT/WITHOUT ROWID. Its ordinary type affinity also does not establish a numeric domain. [SQLite CREATE TABLE](https://www.sqlite.org/lang_createtable.html), [STRICT tables](https://www.sqlite.org/stricttables.html).

Use `STRICT` throughout, explicit boolean checks, nonnegative persisted sizes/sequences, and coherent lease deadlines. STRICT does not replace CHECK constraints. An input `seq = -1` meaning “allocate a sequence” can remain an API convention; it need not be a valid persisted sequence. Constrain roles and closed state vocabularies. Keep intentionally extensible codec/provider/version identifiers extensible.

Use NULL for absence: no expiry, no publication, no parent, and no hard-count override. Empty strings, zero, `-1`, and NULL currently encode different versions of absence. Choose timestamp units globally, preferably integer Unix milliseconds or seconds according to actual precision needs, and convert at the API boundary. Existing RFC3339 text is safe for lexical ordering only if it has a canonical normalization contract; none is enforced by this schema. Do not replace lifecycle timestamps with a single generic timestamp.

Validate JSON where a column is truly always JSON and version its payload. Do not indiscriminately add `json_valid(result)`: some current operation results are hashes or prose. Name those different result forms or put them in an explicit envelope. Keep opaque structured payloads when SQL does not query their internals; normalizing every preference or journal-plan field would add little value.

**6. Medium priority: correct key rotation state and constraints.**

[one_running_link_rotation](/home/vincent/repos/librepaper/librepaper/crates/librepaper/migrations/0001_catalog.sql:296) is unique on `status` within the prepared/running subset. It permits one row of each status. The SQL intended for a global singleton is:

```sql
CREATE UNIQUE INDEX one_active_link_rotation
ON link_key_rotations ((1))
WHERE status IN ('prepared', 'running');
```

The current worker also checks for active work within an immediate transaction, so the probe is a missing schema guarantee, not evidence that two ordinary workers currently bypass that check.

Primary key designation has a concrete application inconsistency: the `all_new == 0` fast path in [access.rs](/home/vincent/repos/librepaper/librepaper/crates/librepaper/src/storage/catalog/access.rs:477) promotes the new key without demoting the old primary. This can occur when there are no links. Recovery chooses a primary by creation time and key-ID ordering, which is an avoidable substitute for explicit state. Demote/promote atomically and enforce one primary, or store the primary key ID in one singleton row. Do not add the unique index without correcting existing writers.

Keep sealed link credentials, their digest uniqueness, and associated-data binding to document/role. The raw encryption keys belong outside SQL; keyring rows are metadata. This review did not audit deployment secrets or historical credentials.

**7. Medium priority: remove derived state with no active consumer.**

[document_results_metadata](/home/vincent/repos/librepaper/librepaper/crates/librepaper/migrations/0001_catalog.sql:367) is populated entirely from `documents.source_format` by two triggers. A whole-repository search found its catalog getter and schema definitions but no calls to the getter. Remove the table, index, triggers, and getter now. If source language and execution engine become independent product settings, make them authoritative fields then; a trigger-derived copy cannot express that independence.

`accounts.erasure_cursor` duplicates `erasure_batches.cursor`. Erasure progress is read through the batch table. Keep one erasure-progress record and drop the duplicate account field.

`checkpoint_asset_sets` currently distinguishes a known-empty asset set from an unknown legacy set. That distinction is useful during migration. For an unreleased baseline in which every checkpoint commits complete references atomically, make completeness mandatory and remove the compatibility marker; if incomplete import is a supported state, retain an explicit completeness field. The stored asset count is separately derivable from edges.

**8. Medium priority: put every persistent table in the migration.**

[source_history.rs](/home/vincent/repos/librepaper/librepaper/crates/librepaper/src/storage/catalog/source_history.rs:682) creates `source_history_gc_encoding_state` during lease sweeping. [cost.rs](/home/vincent/repos/librepaper/librepaper/crates/librepaper/src/server/cost.rs:120) creates `cost_state` during meter initialization. Thus schema version 1 does not describe a fixed persistent schema.

Declare and initialize both centrally. Keep genuine TEMP reservations in connection setup: they represent process-local unacknowledged edits and intentionally disappear on restart. Do not replace them with durable reservations without changing that lifecycle.

The existing fresh-schema test checks only five table names. Add a schema-completeness check that opening the catalog establishes every persistent object, and service initialization or maintenance creates no additional persistent objects. Also add `foreign_key_check` to backup verification: [verify_backup_snapshot](/home/vincent/repos/librepaper/librepaper/crates/librepaper/src/storage/catalog/mod.rs:670) runs only `integrity_check`, which does not check foreign keys. [SQLite PRAGMA documentation](https://www.sqlite.org/pragma.html#pragma_integrity_check).

**9. Medium priority: align indexes with actual work.**

Account erasure filters `comments.author` and `replies.author`, but neither has an author-leading index. The probe uses the comment slug index and scans the reply primary-key index. Batch size limits returned rows, not the number examined. Add `(author, slug, id)` and `(author, slug, comment_id, id)` or their renamed account-identity equivalents.

Execution authorization filters `(slug, execution_epoch)`. The extra unique index is `(slug, conversation_id, execution_epoch)`, but `(slug, conversation_id)` is already unique. It neither adds the desired uniqueness nor supports the epoch lookup directly. If epochs are unique within a document, use `UNIQUE(document_id, execution_epoch)` and add a cleanup index for expired leases.

The automatic checkpoint scheduler orders by `(last_auto_checkpoint_at, slug)` over active, nonpublishing documents. Add that partial index if the scheduler remains a catalog poll. Checkpoint-budget global expiry needs a bucket-leading index; its primary key begins with scope. `quota_retention_candidates` needs a document-leading index for document cascades, because its primary key begins with account/generation.

Remove proven duplication and benchmark the remaining covering-index choices:

| Index | Assessment |
|---|---|
| `journal_segments_sequence` | Duplicates the `UNIQUE(segment_seq)` index |
| `checkpoints_cursor` | Sequence unique index already carries `sha` as the WITHOUT ROWID PK suffix; confirmed by `index_xinfo` |
| `grants_account` / `grants_account_slug` | Both carry account, slug, role due to WITHOUT ROWID suffixes; keep one |
| `guests_account` / `guests_account_slug` | Same suffix principle; inspect plans and keep one |
| `agent_cancellations` duplicate UNIQUE | Repeats the primary-key declaration; SQLite may coalesce it, so this is mainly misleading DDL |
| `comments_cursor` | May be useful as a covering index: unlike checkpoints, comments has a rowid. Do not assume the same redundancy |
| `documents_storage_status` | Storage ID is already unique; covering benefit is the only reason to keep it |
| `links_slug_hash_until` | Hash is already unique and links are few per document; retain only with a useful measured plan |

Do not delete broad owner/listing indexes merely because related partial indexes exist: their predicates differ. SQLite documents prefix-index redundancy, but partial predicates, uniqueness, and covering behavior must also match. [SQLite query planner](https://www.sqlite.org/queryplanner.html).

**10. Design choices for a simpler product model.**

Ownership: keep stable account identity distinct from a display handle. For account-owned documents, the legacy owner key should not remain a second ownership fact. Use an explicit owner-account reference and, only if anonymous ownership is a real product mode, a separately named anonymous-owner identity. Represent open/unowned documents explicitly. Do not introduce a universal principals/organizations/RBAC framework before there is a concrete need.

Accounts: provider IDs as local account IDs are adequate for one-provider-per-account. If linking GitHub and Google to the same account is planned, split `accounts` from `account_identities(provider, subject, account_id)` now, with a unique provider/subject pair. Never merge accounts by email. This is a product-dependent recommendation, not a present defect.

Sharing: grants currently allow multiple role rows per account/document; links allow exactly one per role. If reader/editor are a hierarchy and each account has one effective grant, use `(document_id, account_id)` as the key and store role as a value. If multiple independent capabilities are intentional, keep the current multiplicity and document revocation semantics. Give links their own ID if multiple independently revocable invitations per role are likely. Guest pins should reference the link identity if rotation/revocation should remove the pin; preserve a historical digest without an FK only if pins are explicitly historical records.

Annotations: `comments` mixes discussion, display/source selectors, suggestion state, resolution, and execution context. A wide table is not automatically wrong. Keep core annotation fields together, but consider one optional suggestion row with a closed state machine and a selector representation with explicit kind. Use JSON for a selector payload when SQL never searches its parts; use typed child rows if multi-target annotations are actually planned. Add checks against impossible combinations such as an accepted suggestion without its committed acceptance reference. Define whether deleting a parent comment should also erase other people's replies; CASCADE currently makes that product decision.

Publication: keep the published artifact identity separate from editable source head and checkpoint event. Since annotations preserve publication IDs after republishing, decide whether old publication metadata/artifacts are retainable entities or historical external identifiers. Add a publication table only for an actual lifecycle/query requirement, not just because a column contains an ID.

Journal: retain generation fencing, ordered segments, coverage, bases, reader leases, retirement, and the durable publication preparation protocol. SQLite cannot make a remote object-store write atomic. However, separate a segment's billing owner from coverage. The writer currently stores the first covered document/epoch and aggregate range in `journal_segments` as well as normalized coverage rows; maintenance later updates ownership. Name billing ownership explicitly and make `journal_segment_coverage` the single authority for covered ranges. Empty storage IDs used as global/legacy sentinels should become explicit scope or disappear with the baseline reset.

Retention: keep planned candidate snapshots. A candidate deliberately survives deletion of the checkpoint it describes, so it should not acquire the same cascading checkpoint FK as a live asset reference. `checkpoint_retention` appropriately holds mutable retention state separately from checkpoint content; preserve original ancestry when pruning changes the traversable chain. Keep policy snapshots in jobs so a resumed job does not silently adopt changed configuration.

**Complete table disposition.** Every baseline table is included below; grouping reflects shared recommendations, not a proposal to merge everything in a row.

| Tables | Recommendation |
|---|---|
| `accounts` | Keep; explicit ownership identity; remove duplicated erasure cursor; provider split only if account linking is planned |
| `documents` | Keep; immutable internal PK, unique slug, explicit source/publication identities and ownership |
| `grants`, `links`, `guests` | Keep sharing concepts; choose role/link multiplicity; constrain roles and clarify guest lifecycle |
| `totals` | Keep only as a transactionally maintained, auditable accounting cache |
| `checkpoints` | Keep immutable event identity distinct from content digest; canonical time and persisted sequences |
| `comments`, `replies` | Keep; clarify annotation/suggestion state and author identity; add erasure indexes |
| `pending_deletes` | Keep durable retryable deletion; reference canonical object metadata; keep charged until physical success |
| `catalog_operations` | Keep replay/recovery role; explicit result representation and retention deadline |
| `journal_state` | Keep singleton fencing/head state; explicit absent-head representation |
| `journal_preparations` | Keep unresolved plans; prune/compact resolved evidence safely |
| `journal_segments`, `journal_segment_coverage` | Keep normalized coverage; separate billing attribution and remove duplicated range authority |
| `journal_retirements` | Keep journal-specific retirement preconditions; share physical accounting rather than erase protocol distinctions |
| `account_activity` | Merge into accounts unless independent update cadence is a demonstrated reason to separate |
| `erasure_batches` | Keep sole durable erasure progress; rename to singular per-account job concept |
| `maintenance_jobs` | Keep distinct maintenance reservations; define terminal cleanup |
| `checkpoint_budgets`, `upload_buckets` | Keep separate rate policies if units/semantics differ; explicit owner identity and global expiry indexes/workers |
| `journal_bases`, `journal_manifest_shards` | Keep recovery artifacts; strict identity/size/time invariants |
| `link_keyring`, `link_key_rotations` | Keep metadata and resumable rotation; enforce correct singleton states |
| `object_accounting` | Evolve into authoritative physical objects, separating object identity from charge owner |
| `object_reservations` | Keep prospective allocation distinct from actual measured objects; tie to operation lifecycle |
| `deletion_discovery` | Keep resumable prefix enumeration where the backend cannot enumerate transactionally; remove only when object inventory is complete by construction |
| `journal_readers` | Keep physical read protection; cleanup by expiry; strict IDs |
| `document_results_metadata` | Remove unused derived table and its triggers/index |
| `account_examples` | Keep resumable onboarding slots; slug is reserved before document exists, so absence of document FK is intentional; avoid embedding the number of formats in a permanent identity if examples become configurable |
| `agent_objects` | Keep expiring staged payloads; preserve byte/count bounds; use periodic bounded expiry in addition to opportunistic cleanup |
| `agent_cancellations` | Keep cancellation fences; use operation receipt as authoritative result where possible instead of copying digest/status/result into both tables |
| `agent_execution_leases` | Keep fencing; index actual epoch lookup and prune expired conversations |
| `account_quota_preferences` | Keep versioned opaque payload and optimistic revision; validate JSON envelope |
| `source_history_encodings` | Keep logical digest to physical recipe mapping; explicit codec/version invariants |
| `source_history_objects` | Keep encoding-to-object edges; remove copied object lengths/kinds when canonical objects are authoritative |
| `source_history_checkpoint_files` | Keep checkpoint-to-encoding edges; add actual checkpoint FK after identity unification |
| `source_history_write_leases` | Keep in-flight/read protection currently sharing this table; rename or distinguish purpose; canonical object references and planned allocations |
| `quota_retention_jobs`, `quota_retention_candidates` | Keep durable plans and stale-plan checks; expire terminal history; index document cascades |
| `checkpoint_retention` | Keep mutable protection/ancestry state beside immutable checkpoint identity |
| `document_retention_policy` | Keep explicit enrollment/policy state; merge into documents only if it is mandatory one-to-one |
| `checkpoint_asset_refs`, `checkpoint_asset_sets` | Keep edges with checkpoint/object FKs; eliminate compatibility completeness marker when completeness is mandatory |
| Runtime `source_history_gc_encoding_state` | Keep bounded worker cursor; declare in migration |
| Runtime `cost_state` | Keep versioned durable meter snapshot if persistence is required; declare in migration |
| TEMP `room_edit_reservations`, `admission_documents` view | Keep process-local lifecycle; add counter checks; adapt view to new accounting contract |

**What I would preserve.** Immediate transactions recheck authority and admission at commit; prepared object operations and deletion queues acknowledge cross-store failure; physical deletion does not immediately refund quota; journal generations fence stale writers; reader/writer leases protect concurrent work; keyset cursors and resumable jobs exist; shared-object and account-attribution regressions have meaningful tests. These are sound choices. Simplification should reduce the number of places implementing them, not remove their guarantees.

**Implementation order.**

1. Make the low-risk baseline cleanup: centralize runtime DDL, remove the unused derived table/cursor copy, fix rotation state and nullable IDs, add relevant indexes and SQL invariant tests.
2. Choose identity, timestamp, nullability, ownership, and checkpoint/publication naming conventions. Rewrite the unreleased baseline and catalog mappings together. Use a fresh development database or an explicit export/import process; do not silently run rewritten version-1 SQL against an existing version-1 file.
3. Introduce canonical physical objects and explicit reservations. Establish one admission accounting equation and atomic counters; retain a reconciliation verifier.
4. Replace policy triggers with bounded GC scheduling and real reference FKs. Validate publish-versus-delete, reader-versus-delete, shared-object deletion, failed writes, and restart recovery before removing old machinery.
5. Add bounded terminal-history cleanup and eliminate compatibility paths whose only purpose was old schema support.

**Validation still needed for a redesign.** The passing catalog suite covers atomic receipts, quota deltas, shared assets/chunks, lease expiry, erasure restarts, key rotation, and pagination. Add direct invalid-row tests, full schema inventory, canonical-object/counter reconciliation, restart tests for each terminal cleanup boundary, foreign-key checks on backup, and a large-graph workload that measures SQL work for one admission and one GC batch. A clean fresh install should never need “legacy unknown accounting” to function.

**Product decisions to settle during implementation.** Are linked provider identities planned? Can one account have multiple independent grants on one document? Can a role have multiple links? Must old rendered publications remain accessible? Is anonymous ownership a supported long-term mode? The recommendations above give defaults without requiring those answers to finish this review.

**Verdict: request design changes before freezing the release schema.** No public SQL injection or demonstrated production data-loss path was established by this review. The priority is a smaller, enforceable set of facts: one document identity, one object inventory, one accounting contract, explicit references, and bounded operational history.

Skill reflection: no general change to the code-reviewer skill was needed; its code-first and whole-repository usage checks were applicable here.
