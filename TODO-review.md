# SQLite catalog implementation: restart handoff

Updated: 2026-09-08 (America/Toronto)

## Goal

Finish the **local SQLite** implementation of `docs/specs/catalog.md`. Turso is
explicitly deferred. After functional compliance is approved by an Astra review,
reorganize `crates/librepaper` according to the module tree in the latest user request,
run another complete review, and only then commit everything on
`live-markdown-editor`.

Do not claim completion merely because tests pass. Multiple worker handoffs claimed
findings were fixed, but Astra's code trace showed that several core findings remain.

## Repository state

- Working directory: `/home/vincent/repos/librepaper`
- Intended branch: `live-markdown-editor`
- The worktree is intentionally dirty and contains a large uncommitted implementation.
- Do not reset or discard files. Some pre-existing/unrelated changes are also present;
  the user ultimately requested `commit everything`.
- No final commit has been created.
- Migrations currently extend through schema version 11, including:
  `0003_catalog_contract.sql` through `0011_journal_readers.sql`.

Major new files include `catalog.rs`, `journal.rs`, `backup.rs`, `maintenance.rs`,
and the catalog/journal migrations. Run `git status --short` at restart for the exact
set.

## Last clean verification

Before the second Astra review, the current tree passed:

```text
cargo fmt --all
git diff --check
cargo clippy -p librepaper --all-targets -- -D warnings
cargo test -p librepaper --quiet

library:     616 passed, 0 failed, 1 ignored
integration: 4 passed, 0 failed
```

`target/debug/librepaper` was rebuilt from the current tree immediately before the
second review. Green tests do **not** establish spec compliance.

## Review status

The first complete Astra review returned **REQUEST CHANGES** with 18 findings.
Workers attempted all of them. A second Astra review was started and then interrupted
at the user's request so this handoff could be written. The second review already
confirmed that several worker claims were false or incomplete.

### Confirmed open blockers from the second Astra review

1. **Publication is still not one atomic verified transition.**
   `server.rs` mutates the live room before admission. `room.rs` independently writes
   and commits journal data before the later catalog operation. Checkpoint/head/receipt
   and title changes occur in separate transactions. A staged edit can be observed or
   survive abort. Trace around `server.rs:2865-2909`, `room.rs:3073`,
   `room.rs:3452`, and `journal.rs:2706` (line numbers may move after formatting).

2. **Suggestion acceptance is still split and can leave text/outcome inconsistent.**
   The live CRDT is edited before checkpoint budget admission; journal/checkpoint and
   `finish_suggestion_accept` are separate. Failure can retain accepted text with an
   unresolved receipt. Trace around `room.rs:4364-4410`.

3. **Erasure cursor transition is broken.**
   `run_erasure_pass` retains the old stage cursor after incrementing the stage index.
   For example, a two-field comments cursor can be passed into the three-field replies
   stage, causing a permanent `invalid replies cursor`. Grants/guests may also skip
   earlier keys. There is still no operation-receipt attribution stage. Inspect
   `maintenance.rs:96-119` and all erasure stages.

4. **Backup manifest digest verification uses the wrong contract.**
   Backup hashes raw `journal_manifest_shards` / `journal_state` bytes, while journal
   shard digests are computed from serialization with the embedded digest cleared.
   A valid post-compaction backup may be rejected. Inspect `backup.rs:822-856`,
   `journal.rs:1558`, and `journal.rs:1731`.

5. **Journal reader leases are not wired into production readers.**
   Reader-lease APIs appear to be called only by tests, not `recover_latest` or normal
   read paths. `rewrite_shared_segment` directly deletes an old segment without the
   lease retirement check (`maintenance.rs` near the shared rewrite code, previously
   around line 916).

6. **Checkpoint budgets remain fixed wall-clock buckets, not rolling windows.**
   The implementation still uses `now / 3600` (previously `catalog.rs:2348`).

7. **Existing-room memory growth is still not fully admitted/bounded.**
   Astra found no complete growth check for existing rooms. `resident_bytes` remains
   encoded Yjs length plus summed history payload rather than a demonstrated bound.

8. **Mutation authorization is still incomplete.**
   Comment, reply, and suggestion-accept catalog mutations do not carry and validate
   actor session generation in their final SQL transaction.

The interrupted Astra review had not finished its full matrix or real reproductions.
Restart with a fresh Astra review after fixing the above; explicitly recheck all 18
original findings below rather than assuming unmentioned items are closed.

## Original 18-item Astra matrix

1. Atomic publication and exact startup recovery.
2. Compound suggestion acceptance, rollback, outcome, and retry receipt.
3. Exact retained-payload quota ledger and idempotent peak reservations.
4. Final transactional authorization/generation/link validation for every mutation.
5. Recovery for every journal preparation kind and every crash boundary.
6. Bounded shared-journal compaction and maintenance-reserve use.
7. Conflicting retry detection after compaction / closed epoch semantics.
8. Physical removal of erased payload from shared segments and identity-scoped gates.
9. Reader-safe retirement with complete rooted reachability and protected preparations.
10. Resumable erasure using stable IDs/cursors, cache coordination, and attribution.
11. Complete semantic backup/restore closure and correct digest/decryption validation.
12. Writer-locked, genuinely resumable CLI link-key rotation and consistent keyring use.
13. Local restart must not inherit stale blob room leases.
14. Durable paginated deletion discovery beyond 1,000 objects.
15. Bounded history/room growth and correct durable sequence/flush/checkpoint clocks.
16. Indexed bounded listing for owner/grant/guest/example branches and bounded access data.
17. Rolling/configurable budgets, replacement rate limits, cleanup, recipient limits,
    maintenance capacity, and 1,000-document scale validation.
18. Verified pre-migration/destructive backup tied to all catalog state and identity fsync.

## Implemented work that still needs independent verification

The worktree contains substantial implementations for SQLite migrations, stable storage
IDs, XChaCha link sealing/key IDs, writer locks, journal segments/recovery/compaction,
object accounting/reservations, deletion discovery, account erasure endpoints, backup
and restore, indexed listing, bounded checkpoint tails, checkpoint budgets, key rotation,
and fail-closed catalog reads. Treat these as code to audit, not accepted guarantees.

Tests were added for 100-byte accounting, 600 KiB preflight refusal, >400-link rotation
resume, >1,000 deletion objects, 1,000-document listing, shared-segment rewrite, reader
leases, full-quota compaction, backup freshness, and local restart. Confirm that each test
exercises the production path and does not mask the issue with fixture-only behavior.

## Recommended restart sequence

1. Read `docs/specs/catalog.md`, linked persistence/failover specs, and this file.
2. Inspect `git status`, the full diff, migrations 0003-0011, and current test changes.
3. Fix the eight already-confirmed blockers above with production-path tests.
4. Run formatting, strict all-target Clippy, and the full package test suite.
5. Reproduce quota, backup, restart, erasure, rotation, publication crash, and compaction
   crash cases against a freshly built real binary and file-backed SQLite deployment.
6. Ask a `gpt-6-astra` instance at maximum reasoning for a complete read-only review
   against all 18 findings. Fix every actionable finding and repeat until APPROVE.
7. Only after functional approval, reorganize `crates/librepaper` following the user's
   proposed `cli/`, `server/`, `room/`, `document/`, `storage/`, `auth/`, and `seed/`
   boundaries. Preserve public paths deliberately; update source references in docs.
8. Re-run the complete checks and ask Astra to review the reorganized final tree.
9. When approved, `git add -A`, commit everything on `live-markdown-editor`, verify a
   clean worktree, and report the commit hash.

## Important process correction

Do not accept worker summaries as evidence. Require exact production call-path evidence,
run the claimed reproduction yourself, and have Astra independently inspect the resulting
tree. The previous process lost time because broad workers reported completion while the
old split transaction or fixture-only mechanism was still present.
