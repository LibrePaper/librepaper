# Catalog v2 SQL measurements — 2026-09-13

These measurements cover the SQL matrix in specification section 24. They do **not** establish completion of the migration or acceptance of its runtime protocols. The physical journal comparison below is a separate runtime observation. The seeded root/physical-inventory audit described below also passes.

The runner creates disposable catalogs from the normative DDL, enables foreign keys and WAL with `synchronous=FULL`, and validates reference counters, foreign keys and SQLite integrity before recording each case. No application database was opened. Builds and tests were paused during the measurements. Hardware: AMD Ryzen AI 7 PRO 450, 16 logical CPUs; Linux 6.18.45; Python 3.14.6; SQLite 3.53.1. Storage latency is part of the wall times; these are single observations, not controlled repeated trials.

Reproduce a case from the repository root:

```sh
uv run --no-project --python 3.14 docs/specs/benchmark-catalog-v2.py \
  --workload documents --reuse 100 --output /tmp/catalog-v2-documents100.json
```

The runner uses empty physical objects and synthetic tree rows. Object IDs, reference counts, and SQL indexes are real; object encoding, manifest completeness, authorization, Rust admission, hashing and physical payload I/O are excluded. The reuse percentage controls adjacent-checkpoint reuse of physical IDs, not deduplication by digest. Prepared and terminal operation counts are zero in all cases. Consequently these timings are SQL costs, not end-to-end checkpoint latency.

| Workload | Reuse | Reference rows | Object rows | Catalog MiB | Build seconds | Checkpoint p95 ms | Bounded deletion ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| [small](catalog-v2-sql-matrix-20260913.json) | 0% | 640,000 | 640,000 | 592.1 | 39.24 | 19.30 | 51.13 |
| [small](catalog-v2-sql-matrix-20260913.json) | 90% | 640,000 | 76,500 | 204.7 | 28.01 | 14.88 | 12.92 |
| [small](catalog-v2-sql-matrix-20260913.json) | 100% | 640,000 | 12,800 | 153.7 | 54.07 | 26.97 | 35.82 |
| [histories](catalog-v2-sql-matrix-20260913.json) | 0% | 6,400,000 | 6,400,000 | 5,918.2 | 336.99 | 15.57 | 49.45 |
| [histories](catalog-v2-sql-matrix-20260913.json) | 90% | 6,400,000 | 765,000 | 2,044.1 | 210.56 | 9.03 | 10.18 |
| [histories](catalog-v2-sql-histories100-20260913.json) | 100% | 6,400,000 | 128,000 | 1,534.4 | 212.90 | 8.72 | 10.63 |
| [documents](catalog-v2-sql-documents0-20260913.json) | 0% | 6,400,000 | 6,400,000 | 5,945.3 | 497.60 | 11.95 | 7.04 |
| [documents](catalog-v2-sql-documents90-20260913.json) | 90% | 6,400,000 | 1,270,000 | 2,373.4 | 367.63 | 8.96 | 29.00 |
| [documents](catalog-v2-sql-documents100-20260913.json) | 100% | 6,400,000 | 640,000 | 1,906.1 | 248.76 | 5.24 | 5.70 |
| [document-ceiling](catalog-v2-sql-document-ceiling-20260913.json) | 100% | 1,048,576 | 16,384 | 250.7 | 16.41 | 796.13 | 680.60 |
| [deployment-ceiling](catalog-v2-sql-deployment-ceiling-20260913.json) | 100% | 8,388,608 | 131,072 | 2,002.8 | 311.90 | 1659.37 | 500.62 |

Catalog size includes secondary indexes after a WAL checkpoint. The reports separately record sampled WAL size; it is sampled after each document, not a claim about the true peak. Deletion uses the smaller of 32 checkpoints, 32,768 edges, and the available non-head checkpoints: 32/4,096 edges for small and history cases, 9/576 for many documents, and 2/32,768 for the ceiling cases.

The largest deployment case used 2,100,101,120 catalog bytes and sampled 89,519,392 WAL bytes. Its checkpoint SQL transactions averaged 608.78 ms, with p95 1,659.37 ms and a maximum of 3,224.82 ms. Deleting 32,768 reference edges took 500.62 ms. The single-document ceiling used 262,881,280 catalog bytes. These costs must remain visible when deciding the final release budgets; the provisional maxima are not evidence of acceptable interactive latency.

Quota and reference admission probes address the document and owner primary keys and the one-row server singleton. SQLite may describe the singleton access as `SCAN s`; it is one row, not a history or owner scan. GC candidate selection uses `objects_gc`; the live-root probe uses the covering `objects_live_roots` index. The latter has no matching roots in these synthetic fixtures, so it verifies plan selection, not populated-root traversal performance. JSON reports include wall times, query plans, and approximate SQLite VM-step intervals.

The first five cases are preserved in `catalog-v2-sql-matrix-20260913.json`, an intentionally interrupted multi-case run. Those five cases finished and were serialized; the file is not a complete matrix report and predates the runner's completion/time/schema-hash fields. The six separately completed reports supply the remaining cases. This directory contains all eleven requested SQL cases; it does not turn the interrupted report into a complete execution or invent missing metadata.

## Physical journal comparison

The [physical trace report](catalog-v2-physical-journal-20260913.json) records one passing debug-build run of `spec24_per_document_journal_comparison_trace`. It interleaves 50 full-state updates for each of 100 documents (plus one fixture document), writes to real temporary FsStore directories with durable writes enabled, and compares final source text and CRDT state vectors after recovery.

| Measurement | Per-document v2 | Mixed coordinator baseline |
|---|---:|---:|
| Physical segments | 5,000 | 40 |
| Physical bytes | 1,987,599 | 1,962,999 |
| Write wall time | 6,387 ms | 301 ms |
| Recovery wall time | 398 ms | 65 ms |

The v2 path includes SQLite admission and publication for each document update. The baseline batches through the bounded mixed coordinator and FsStore; it is not the complete old runtime. Its recovery extracts the last full-state record per document, while v2 follows its catalogued chain. This comparison exposes the cost of separate durable objects and must not be described as an apples-to-apples throughput benchmark. The test configures durable writes but does not count fsync syscalls. These results do not establish release performance acceptance or replace the independent randomized root audit.

## Seeded physical inventory audit

At integration commit `de533c55`, `cargo test --offline -p librepaper --lib tests::catalog_v2_randomized -- --nocapture` passed its 48-step seeded test. It combines source checkpoints, journal acknowledgements and reopen, labels, retention, GC, and asset naming/removal. After each step it independently hashes and measures the physical inventory, decodes trees and recipes to reconstruct exact checkpoint closures, verifies recovered asset roots, and reconciles document/account/deployment counters. This is one reproducible seed on a small document, not exhaustive concurrency or failure-injection coverage.
