# Frugal validation, 2026-09-23

This investigation uses disposable local databases and synthetic clients. It
does not establish small-VPS capacity or representative production storage
costs. The worktree started at `139b9852c90cef37fb251f33f3ea20e534b455ad`.
Three inexpensive parallel subagents authored the harnesses and inventory.
They ran no tests or checks; all validation and experiments were run centrally.
The remaining deployment work is tracked in [SPEC-frugal.md](../../SPEC-frugal.md).

## Sustained writes and a stalled database

Release binaries, eight Tokio workers, PostgreSQL 17.11 on the same
16-logical-CPU Ryzen AI 7 PRO 450 host with about 54.6 GiB RAM. PostgreSQL used
128 MB shared buffers, fsync, full-page writes and synchronous commit enabled.
The application used its default 20-connection pool and 64 MiB retained and
scratch budgets. Socket admission limits were explicitly raised to 2,048 for
one local principal. Each document had a writer and an editor-authorized
observer. The clients append pseudorandom text to synthetic Loro documents;
this does not exercise browser rendering or a realistic project file tree.

| Measurement | Ordinary writes | Write stall |
| --- | ---: | ---: |
| Documents / sockets | 100 / 200 | 1,000 / 2,000 |
| Text added per edit | 8 KiB | 4 KiB |
| Scheduled edits per document per second | 2 | 2 |
| Scheduled traffic duration | 40 s | 60 s |
| Write lock held | None | 45 s |
| Updates eventually relayed and durable | 8,000 | 120,000 |
| Actual traffic duration | 40.0 s | 122.5 s |
| Actual update throughput | 200/s | 980/s |
| Relay median / p99 | 11.5 / 102.7 ms | 97.1 / 6,322 ms |
| Final drain after traffic | 5.28 s | 6.68 s |
| Peak retained pending bytes | 51,468,090 | 67,108,278 |
| Retained budget | 67,108,864 | 67,108,864 |
| Peak scratch reservation | 23,870,652 | 4,942,567 |
| Refused attempts / distinct updates later recovered | 0 / 0 | 20,853 / 3,482 |
| Combined server-and-client process RSS peak | 335 MiB | 1,798 MiB |

Every final client vector was found in the durable database rows, and both
pending pools returned to zero. RSS was sampled independently every 100 ms,
including while a round waited for retries. PostgreSQL's separate Docker
working-set sample peaked at approximately 271 MiB; this is a shared-cluster
metric, not per-database RSS. The two memory peaks need not be simultaneous.

The stall holds an `EXCLUSIVE` lock on `document_updates`, which delays writes
while allowing reads and writer-lease probes. Its control transaction consumes
one additional application-pool slot in the writer harness. The workload is
closed-loop and synchronized: one refused update delays the next round for
every document. Retries reuse the same payload every second, unlike the
browser's exponential backoff. Thus this checks resource ownership and eventual
recovery under pressure; it is not a maximum-throughput or fairness benchmark.

Final-update durable-observation p95 was 5.67 s and 6.54 s, respectively.
These are upper bounds measured by database polling after traffic ends, not
the latency distribution of every durable notification during the run. In
particular, the final-update statistic does not describe edits delayed during
the initial stall. The worst relay delay during the stall was 38.4 s.

**Decision:** retain the separate pending/scratch budgets and existing flush
scheduler. The cap held and the deployment recovered. Do not increase the
defaults on the strength of combined client/server RSS or a synthetic stall.
Run the same workload with a separate load-generator host before sizing a VPS.

Raw observations: [ordinary writes](writer-baseline.log), [write stall](writer-slow.log),
[PostgreSQL samples](postgres-stats-writer.jsonl), and [host](host.json).

## Authorization, interactive traffic and reconnects

The mixed harness uses signed session cookies, one owner/editor per document,
32 concurrent HTTP project reads, and up to 128 simultaneous reconnects.
Socket ceilings and the per-principal request allowance are raised explicitly;
the application retains its default 20 database connections and 64 concurrent
HTTP work slots. A separate control connection stalls update writes for
45 seconds. The tiny synthetic edits do not populate a realistic project tree.

Idle authorization runs for 60 seconds with synchronized 25-second heartbeats.
A separate ten-second phase sends five pings per second per socket to exercise
the one-second cache. PostgreSQL execution time excludes application and pool
waits; SQL classified as authorization during mixed traffic also includes
catalogue reads from HTTP requests. Transaction-begin wait telemetry does not
cover every single-statement query.

The final 1,000-socket run passed:

| Measurement | Result |
| --- | ---: |
| Idle pings / authorization queries over 60 s | 2,000 / 10,000 |
| Idle authorization cumulative SQL time / ping p99 | 237 ms / 180 ms |
| Frequent pings / authorization queries over 10 s | 50,000 / 45,000 |
| Frequent authorization cumulative SQL time / ping p99 | 1,050 ms / 158 ms |
| Mixed edits / HTTP project reads completed | 60,000 / 60,000 |
| Edit / HTTP failures | 0 / 0 |
| Mixed traffic duration / final drain | 90.5 s / 4.25 s |
| HTTP median / p95 / p99 | 6.99 / 8.73 / 10.30 ms |
| Edit-plus-ping round-trip p95 | 224 ms |
| Transaction-begin mean / maximum wait | 0.432 / 3.196 ms |
| Reconnects admitted / HTTP 429 refusals | 322 / 678 |
| Successful reconnect p99 | 67.5 ms |

Five authorization queries per expired cookie refresh matched the expected
path. The frequent phase performed approximately 9,000 refreshes across
50,000 messages, consistent with about 82% cache hits; this is inferred from
SQL counts, not a direct cache-hit counter. Both pending pools drained to
zero after mixed traffic. PostgreSQL container working set peaked near 266 MiB.
The HTTP requests follow the edit burst within each round; housekeeping writes
run concurrently. This is a bounded, closed-loop workload, not independent
open-loop HTTP traffic, and the edit round trip is not durable-save latency.

Reconnect offered 128 simultaneous attempts against the unchanged 64-slot
HTTP work limit. Every failure was HTTP 429; no retries were attempted.
The server stayed live with warm rooms, and success means an initial state or
row response arrived, not full browser import/render or cold-start recovery.
These results do not claim that all clients reconnect on their first attempt.

**Decision:** retain the pool and flush concurrency defaults. There was no
interactive failure during the write stall, and these local SQL costs do not
justify an authorization or writer-lease redesign. Validate browser retry
backoff, cold replay and recovery after an actual restart on the target host
before raising admission limits. See [raw mixed observations](mixed.log) and
[PostgreSQL samples](postgres-stats-mixed.jsonl).

Initial harness failures used
the wrong HTTP endpoint/client header and assumed synchronization always
started with `doc-state`; persisted history can start with `doc-rows`.
These are harness defects, not evidence of application failures. The final
harness checks HTTP access before starting the timed phases.

## Storage and encrypted recovery

The drill uses the existing backup/restore CLI, an operator-managed age
recipient and identity, and separate empty databases. The original plaintext
backup is removed before decryption. Database signatures and object bytes are
compared, followed by application replay of every document head and labeled
historical frontier.

It exposed two defects, fixed in this worktree:

- `pg_dump` did not expand the connection URI passed only via `PGDATABASE`.
  `pg_restore` also needs `--dbname` to restore rather than emit SQL. Both
  commands now receive an explicit, password-free connection URI; the password
  stays in the child environment. Regression tests cover authority and query
  passwords and preservation of other URI parameters.
- Simulated seed history started exporting after file-map, asset and main-file
  setup operations. Those operations never reached the log, so cold replay
  produced an empty document even though backup checksums matched. The first
  seed update now includes the complete initial history. This repairs future
  seeds; it does not modify existing deployments or reconstruct their missing
  operations.

The initial failures are retained in [backup failure](recovery-initial-failure.log)
and [application replay failure](replay-initial-failure.log).

The corrected drill passed: five document heads and all 82 labeled historical
frontiers replayed identically, with nonempty source text at every head. All
five asset objects also matched byte for byte. See [recovery](recovery.log),
[application replay](replay.log), [SQL inventory](storage.tsv), and
[filesystem inventory](filesystem.jsonl).

| Stored component | Measured bytes |
| --- | ---: |
| 186 update-row payloads | 52,296 |
| Asset objects, logical / allocated file bytes | 58,470 / 61,440 |
| All user relations, including indexes and TOAST | 884,736 |
| Index subset of those relations | 507,904 |
| Whole database, including system catalogs and other allocation | 9,139,891 |
| PostgreSQL custom-format backup dump | 81,323 |
| Compressed backup bundle / age-encrypted bundle | 49,187 / 49,387 |
| Encrypted bundle allocated file bytes | 53,248 |

These rows overlap and must not be summed indiscriminately. The five starter
assets are identical: global deduplication would save 46,776 logical bytes in
this fixture, insufficient evidence for a shared-object ownership redesign.
Current and retired snapshots, label archives, and proposal payloads are zero
in this fixture. Their inventory queries are present, but backup recovery of
those object classes was not exercised by this particular drill. The final [WAL inventory](cluster-wal-bytes.txt) was 503,316,480 bytes for
the whole disposable cluster, including the much larger benchmark databases.
It must not be attributed to this small recovery fixture. WAL allocation
changes as benchmarks and checkpoints run.

**Decision:** keep per-document asset ownership and full source history.
The useful implementation changes were the backup connection fix and complete
seed history. Use the documented external age workflow, private staging and
separately retained recovery keys; do not describe the backup CLI itself as
encrypted. Production inventory, key custody and recovery-time objectives
remain deployment-specific work requiring a named target and its data.

## Reproduction and validation

Use a disposable PostgreSQL 17 instance with `pg_stat_statements` preloaded and
enabled in each benchmark database. Build from this worktree with
`cargo test --release -p librepaper --lib --test writer_socket_bench --test frugal_mixed_bench --test frugal_restore_verify`.
The benchmarks are ignored by default. Run performance workloads separately
from compilation and other test suites.

The final mixed run used `FRUGAL_MIXED_SOCKETS=1000`,
`FRUGAL_MIXED_SECONDS=60`, and `FRUGAL_MIXED_DB_STALL_SECONDS=45`,
with cookie authentication.

The writer runs used `WRITER_SOCKET_BENCH_MODE=durable`,
`WRITER_SOCKET_BENCH_RATE=2`, and `WRITER_SOCKET_BENCH_REPS=1`. Ordinary writes
used `DOCUMENTS=100`, `SECONDS=40`, `UPDATE_BYTES=8192`; the stall used
`DOCUMENTS=1000`, `SECONDS=60`, `UPDATE_BYTES=4096`, `DB_STALL_SECONDS=45` (all
with the `WRITER_SOCKET_BENCH_` prefix). See the
[writer guide](../writer-socket-bench/README.md),
[mixed guide](../frugal-mixed-bench/README.md),
[inventory guide](../frugal-storage-inventory/README.md), and
[recovery drill](../frugal-recovery/README.md).

Central validation: 494 Rust library tests passed, 96 ignored; the three
benchmark/drill integration tests are deliberately ignored in the ordinary
test invocation and were run separately against disposable databases. The
recovery drill and application replay passed. Formatting and shell/JavaScript
syntax checks passed. Clippy passed for the release workspace and all targets with warnings denied.

Benchmark executable hashes are retained with the observations. The seed and
backup fixes do not alter the measured authorization, writer, flush or budget
paths. No cache redesign, lease amortization, extra flush pool, global asset
deduplication, history truncation or sharding is justified by these runs.
