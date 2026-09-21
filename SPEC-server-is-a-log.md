# Log implementation: status and measurements

The full specification this implements is not in this file. It is the
564-line `SPEC-server-is-a-log.md` at commit `3391479c`; the section
numbers cited throughout the code (§4.5, §8.4, §9.2 and 40 others) refer
to it. Restoring it beside this status note would make those citations resolve
again.

## Correctness work already implemented

Rechecked against the source on 2026-09-21. Both former checklist items are
removed from outstanding work:

- Joins carry `durableVector` separately from the head vector, and successful
  persistence broadcasts `doc-durable` to editors. The browser derives save
  status from durable coverage, independently of transport acknowledgements.
  See `crates/librepaper/src/log/sequencer.rs`, `server/socket.rs` under the same
  crate, and `web/src/lib/project-session.js`. Regression coverage exists in
  `crates/librepaper/src/log/tests.rs` and
  `web/tests/unit/project-session.mjs` for buffered reconnects and failed flushes.
- `web/tests/integration/slow-subscriber-recovery.mjs` exercises a real
  slow-subscriber close, automatic reconnect, concurrent offline edits,
  IndexedDB recovery after page reload, and durable state after server restart.
  This closes the missing-test task; it is not a claim that the test was rerun
  during this document review.

## Measured

`storage::postgres::benchmarks` holds the release benchmarks, each
`#[ignore]`d and gated on `LIBREPAPER_BENCHMARK_POSTGRES_URL`:
`typing_throughput_release_benchmark` (§14.1), and
`compaction_cost_release_benchmark`, which times §8.4 one stage at a time
on the call sequence `worker::compact` uses, with an editor typing
throughout and a 5 ms RSS sampler. `update_import_cost_probe` and
`batched_import_cost_probe` beside them price Loro's import directly and
need no database. Shapes come from `LIBREPAPER_COMPACTION_BENCHMARK_EDITS`
as `edits:seed_kib`; the report goes to
`LIBREPAPER_COMPACTION_BENCHMARK_OUTPUT`.

What those runs establish, as context for future capacity work
(2026-09-20, release, PostgreSQL 16.15, one document on an idle process):
a cold build of a 1 MiB log is about 8 ms and every other compaction stage
is under 35 ms. Those runs do not justify a compaction redesign; they do not
establish costs for long-lived documents or concurrent workloads.
The transient peak of a build is 3 to 27 MB for logs of 1 to 3.5 MiB.
The implementation now reserves `BUILD_TRANSIENT_EXPANSION` (8x log bytes)
during import in addition to the resident estimate; the former missing
transient-reservation task is complete. `MAX_UPDATE_BYTES` caps an individual
update at 4 MiB and refuses an 8 MiB upload; it is not the document-size ceiling.

Two rules the harness earned and should keep: a benchmark asserts that its
own run happened, and a failure is a result to record rather than a panic,
which is how an unreadable 2 MiB document was found at all.
