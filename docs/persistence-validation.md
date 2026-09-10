# Persistence validation

The local persistence implementation uses a durable SQLite catalogue and shared
immutable journal segments. This page records validation of that implementation,
separately from the broader release targets in [the spec](specs/persistence.md).

## Reproduce the workloads

Run from the repository root after preparing the normal Rust build prerequisites
(`make wasm` and the web build). Reports use absolute paths because Cargo runs
unit tests from the crate directory.

```sh
mkdir -p docs/benchmarks
LIBREPAPER_BENCH_REPORT="$PWD/docs/benchmarks/persistence-normal.json" \
  cargo test -p librepaper --lib persistence_capacity_workload -- --ignored --nocapture
LIBREPAPER_BENCH_DOCUMENTS=1000 \
  LIBREPAPER_BENCH_REPORT="$PWD/docs/benchmarks/persistence-1000.json" \
  cargo test -p librepaper --lib persistence_capacity_workload -- --ignored --nocapture
LIBREPAPER_DEFAULTS_REPORT="$PWD/docs/benchmarks/persistence-defaults.json" \
  cargo test -p librepaper --lib persistence_default_limits -- --ignored --nocapture
```

The capacity workload defaults to eight documents, 100 KiB of initial source per
document, and three edit/save rounds. `LIBREPAPER_BENCH_DOCUMENTS`,
`LIBREPAPER_BENCH_BYTES`, `LIBREPAPER_BENCH_EDITS`, `LIBREPAPER_BENCH_ROOMS_MAX`, and
`LIBREPAPER_BENCH_ROOMS_BYTES_MAX` override those inputs. Each document has its own
owner and an attached editor channel. Updates use the normal Yjs room admission
path. Each round dirties all rooms, persists with four concurrent saves, and
requires a durable acknowledgement for every editor. Initial checkpoints are
separate from the measured edit/save rounds.

Both the object store and SQLite explicitly enable durability, including in test
builds. Recovery drops the original deployment and opens fresh catalogue,
object-store, journal, and room instances. It checks every document's latest text
and verifies that a recovered writable room refuses oversized source without
changing its contents. The default-limit workload fills resident capacity with
dirty documents near the default source-size ceiling and checks that admission
refuses additional rooms without discarding existing work.

## Recorded run: 2026-09-10

Machine: AMD Ryzen AI 7 PRO 450, 16 logical CPUs, approximately 54 GiB RAM,
Linux 6.18.45, local NVMe storage. Compiler: Rust 1.99.0-nightly
(`11177f223`, 2026-08-02). These measurements use Cargo's unoptimized test
profile, not a release build. They are a reproducible development baseline,
not production throughput guarantees.

| Workload | Edit/save rounds | Save round durations | Reopen and verify | Process peak RSS |
| --- | --- | --- | --- | --- |
| [8 documents](benchmarks/persistence-normal.json), 100 KiB each | 3 | 272 / 269 / 265 ms | 300 ms | 92.4 MiB |
| [1,000 documents](benchmarks/persistence-1000.json), 100 KiB each | 3 | 32,478 / 32,883 / 33,154 ms | 36,465 ms | 335.6 MiB |

All edits received their durable acknowledgements, every document recovered its
latest content, all staging and queue reservations were released, and catalogue
execution reported zero failed jobs. The 1,000-document run retained 1,999
segments totaling 411,314,478 bytes. It did not run long enough to trigger
compaction; compaction and retirement have separate regression coverage below.

The roughly 33-second save rounds do **not** establish the spec's fifteen-second
dirty-age target at 1,000 simultaneously dirty documents. Scheduling correctness
is tested separately; release-mode throughput and the full network workload
still need measurement before claiming that target at this scale.

The [default-capacity stress run](benchmarks/persistence-defaults.json) admitted
128 dirty documents near 4 MiB each, then refused further admission at the
resident-byte limit, before reaching the 200-room count limit. All admitted
contents remained intact. Peak process RSS was 610.1 MiB; the 512 MiB setting
bounds the room admission estimate, not total process RSS. The reported
resident estimate was 536,883,050 bytes, slightly above the limit after the
admitted rooms' metadata edits. This run is not evidence of a strict 512 MiB
process-memory ceiling.

## Regression coverage

| Guarantee | Executable evidence |
| --- | --- |
| Scheduled saves respect the write floor and continuous-edit deadline | `journal_scheduled_saves_obey_floor_and_dirty_deadline` |
| Deletion-only edits survive restart despite an unchanged state vector | `deletion_only_edit_recovers_with_unchanged_crdt_state_vector` |
| Missing/corrupt committed objects fail recovery visibly | `recovery_fails_closed_when_a_committed_segment_is_missing`, `recovery_fails_closed_when_a_committed_segment_is_corrupt` |
| Failed object writes leave admission retryable | `failed_segment_write_releases_admission_and_retry_succeeds` |
| Recovery permits generation gaps between complete snapshots | `recovery_accepts_nonconsecutive_snapshot_sequences_and_compacted_bases` |
| Uncertain deletions cannot release accounting merely because a retry sees absence | `uncertain_deletion_retry_syncs_surviving_ancestor_before_absent`, `uncertain_unlink_keeps_retrying_until_parent_sync_succeeds` |
| Prepared journal writes reconcile after restart | `restart_reconciles_complete_and_aborted_preparations` |
| Compaction remains possible at full ordinary quota | `compaction_borrows_maintenance_headroom_at_full_quota` |
| Shared-object deletion preserves other documents and active readers | `shared_segment_rewrite_physically_excludes_erased_identity`, `journal_reader_lease_blocks_retirement_until_release` |
| Completed backups restore objects, catalogue, secrets, and identity | `backup_manifest_is_written_last_and_verifiable`, `local_backup_restores_catalog_objects_secrets_and_identity` |

The new failed-write fixture induces a filesystem write failure; it does not
claim to exhaust an actual disk. Directory-sync failures are injected per call,
so concurrently running tests cannot change each other's filesystem behavior.

## Measurement boundaries

These are in-process room workloads with attached editor channels, not 1,000
network clients. They report save-round durations, total replay/reopen time,
stored bytes, committed segments, catalogue execution counters, peak journal
staging bytes, and Linux process RSS high-water marks. RSS includes the test
harness and retained expected text; it is not a heap allocation measurement.

The workload does not benchmark network traffic, annotations, timed scheduler
latency, backup traffic, every checkpoint reason, or sustained maintenance at
full quota. SQL statement counts and physical filesystem write counts are not
instrumented. The broader release matrix still requires those measurements and
process-kill/ambiguous-commit fault campaigns. Existing targeted regressions are
evidence for their individual guarantees, not a substitute for that campaign.
