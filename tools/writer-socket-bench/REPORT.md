# Writer verification: measured assessment

Measured on 2026-09-20 (America/Toronto). The deployment-wide serialization
point is real, but these runs do not establish an urgent local-database
bottleneck or justify changing production ownership behavior.

## Live socket results

Each initially empty document had one active writer and one editor-authorized
observer. Writers sent tiny synthetic Loro text edits at five updates/second,
in synchronized rounds, for five seconds. There were two repetitions per shape.
Connection setup was outside the measurement window and admitted at most 32
concurrent handshakes. Every expected relay arrived; both full test runs passed.

| Active documents | Total sockets | Observed updates/s | Median send-to-relay | p99 send-to-relay | p99 scheduled-to-relay |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 2 | 4.99–5.00 | 0.59–0.64 ms | 36–40 ms | 36–40 ms |
| 10 | 20 | 49.95–49.96 | 1.14–1.31 ms | 34–37 ms | 34–37 ms |
| 100 | 200 | 499.41–499.69 | 3.26–3.29 ms | 36–72 ms | 39–75 ms |
| 1,000 | 2,000 | 4,994.77–4,997.05 | 26.95–28.78 ms | 65–70 ms | 79–85 ms |

Each repetition at 1,000 documents relayed 25,000 updates and counted exactly
25,000 matching probe SQL statements. The corresponding counts matched updates
at the smaller shapes too. The check was exercised, not bypassed.

Raw results: [1–100 documents](baseline.json) and
[1,000 documents](thousand-documents.json). The first attempt opened all sockets
concurrently and hit the HTTP work-concurrency guard at 100 documents; see
[the failed attempt](initial-admission-failure.log). The retained harness
throttles setup rather than changing that guard.

## What the isolated probe adds

The [separate probe benchmark](../writer-lease-bench/REPORT.md) excludes all
authorization, CRDT, socket and persistence work. With phase-staggered arrivals
at 5,000 probes/second, three focused local repeats sustained about 4,999/s,
with p99 mutex wait of 0.28–0.30 ms. Other runs under host contention had much
worse tails, up to about 169 ms scheduled-to-completion; the machine was not
an isolated performance host.

Adding a synthetic server sleep requested at 1 ms produced approximately
2.07 ms measured query service time. That serialized path sustained only about
483/s when offered 5,000/s, with a 9.35-second drain tail for a one-second arrival
window. This demonstrates the mechanism under slow probes, not a measured
production-network problem.

## Interpretation and next decision

The synchronized socket workload can make 1,000 writers queue on the single
connection at once, unlike the phase-staggered microbenchmark. Its larger relay
latencies include that queue, authorization, scheduling, CRDT ingestion and TCP
delivery. There is no probe-disabled or cached-probe comparison, so these results
do not isolate the fraction of socket latency an optimization would remove.

Leave the writer-check architecture alone for now. Revisit if the actual VPS
shows probe waits or slow database round trips dominating the latency budget.
If it does, compare sharing a probe among concurrent callers or a short lazy
validity cache against the current behavior, with explicit failover tests. The
durable epoch fence is separate from this liveness check; retaining that fence
does not by itself validate a longer stale-relay window.

These results weaken the earlier claim that this was likely the biggest simple
win. Reader refresh scheduling remains a smaller change with a clearer reduction
in unnecessary work. No production behavior was changed in this investigation.

## Scope and reproducibility

- Host: AMD Ryzen AI 7 PRO 450, 16 logical CPUs, approximately 56 GB RAM.
- PostgreSQL 17.11, disposable `postgres:17-alpine` container on the same host,
  loopback TCP, `pg_stat_statements` enabled. This is not a small-VPS benchmark.
- Release build, eight Tokio worker threads, default catalogue connection pool.
  Socket limits were overridden to 2,048 to admit one local synthetic account.
- Frozen application commit: `3352432d5b1ef071f6a8146a27ec6fd92c26f645`.
  Concurrent unrelated workspace changes prevented a reliable working-tree
  build. The frozen commit has the same probe and mutex path inspected in the
  workspace; later unrelated changes are not represented by these results.
- Test executable SHA-256:
  `bd711c854877b8ba8d8d14b081aa3dc5be0c0afadfda960536f4aad366bdc824`.
- Short relay windows only: no periodic housekeeping, 30-second age flushes,
  sustained durability measurements, browser rendering, projection fetches,
  realistic document sizes or takeover scenarios. Small-shape p99 values have
  few samples and include first-update/startup effects.
- [Harness and commands](README.md). For the 1,000-document run, set
  `WRITER_SOCKET_BENCH_DOCUMENTS=1000`; all other workload settings remain the same.

Validation: both workload suites passed; the frozen-checkout integration target
and the standalone probe harness passed Clippy with warnings denied. Rust
formatting and whitespace checks passed. Disposable benchmark databases and
containers were removed after recording the results.
