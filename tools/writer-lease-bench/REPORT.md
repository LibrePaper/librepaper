# Writer-lease verification investigation

This benchmark isolates the current server shape in
`crates/librepaper/src/storage/postgres/ownership.rs`: one dedicated
PostgreSQL session is held behind one Tokio mutex, and each admitted operation
executes the exact production query `SELECT 1`. It does not exercise WebSocket
authorization, document persistence, CRDT work, or relay traffic, so these
numbers are verification-path measurements rather than application capacity.
The harness reproduces the mutex and query, not the `WriterLease` type itself;
it does not acquire the advisory lock or test ownership loss.

The source under test was unchanged and had SHA-256
`038c329e56fc9a65faeeb81330342282c8981ec030d2cebfb3050bae1a39a151`.
The harness is [src/main.rs](src/main.rs), with commands and options in
[README.md](README.md).

## Environment and method

The database was a new disposable `postgres:17-alpine` container, exposed only
on `127.0.0.1:55433`, with database `lease_bench`; the existing PostgreSQL
containers and databases were not used. The server reported PostgreSQL 17.11
on x86_64 Alpine. The host is an AMD Ryzen AI 7 PRO 450 with 16 logical CPUs
and 56 GB RAM. The benchmark used an 8-thread Tokio runtime, one PostgreSQL
connection, 500 ms warmup, and either two 2 s repetitions or one 5 s
repetition. The focused repeat used three 5 s repetitions.

Each sample records mutex wait, query service, actual attempt-to-completion,
and scheduled-arrival-to-completion. Paced callers are phase-staggered across
the interval. Throughput uses `max(measurement window, observed drain time)`;
the CSV separately reports the drain tail.

Commands:

```sh
cargo check --manifest-path tools/writer-lease-bench/Cargo.toml
WRITER_LEASE_BENCH_OUT=tools/writer-lease-bench/baseline.csv \
  cargo run --release --manifest-path tools/writer-lease-bench/Cargo.toml -- \
  --url postgresql://postgres:writer-lease-bench@127.0.0.1:55433/lease_bench
WRITER_LEASE_BENCH_OUT=tools/writer-lease-bench/baseline-5s.csv \
  cargo run --release --manifest-path tools/writer-lease-bench/Cargo.toml -- \
  --url postgresql://postgres:writer-lease-bench@127.0.0.1:55433/lease_bench \
  --warmup-ms 500 --measure-ms 5000 --repetitions 1
WRITER_LEASE_BENCH_OUT=tools/writer-lease-bench/focused-5s-quiet.csv \
  cargo run --release --manifest-path tools/writer-lease-bench/Cargo.toml -- \
  --url postgresql://postgres:writer-lease-bench@127.0.0.1:55433/lease_bench \
  --warmup-ms 500 --measure-ms 5000 --repetitions 3 \
  --only-concurrency 1000 --only-pace-ms 200
WRITER_LEASE_BENCH_OUT=tools/writer-lease-bench/induced-1ms.csv \
  cargo run --release --manifest-path tools/writer-lease-bench/Cargo.toml -- \
  --url postgresql://postgres:writer-lease-bench@127.0.0.1:55433/lease_bench \
  --measure-ms 1000 --repetitions 1 --server-delay-ms 1
```

## Results

The two-repetition local baseline is in [baseline.csv](baseline.csv). In the
closed-loop case, the median query service time was 17–20 µs. At 1,000
concurrent callers, median mutex wait was 19.0–19.8 ms and p99 end-to-end was
42.7–195.9 ms across the two repetitions. At 100 callers, median mutex wait
was 1.85–2.04 ms and p99 end-to-end was 2.82–2.85 ms.

The focused 5 s repeat is in [focused-5s-quiet.csv](focused-5s-quiet.csv):

| offered load | repetitions | observed/s | p50 mutex wait | p99 mutex wait | p99 scheduled→done |
|---:|---:|---:|---:|---:|---:|
| 5,000/s (1,000 callers × 5/s) | 3 | 4,998–5,000 | 78–92 µs | 281–304 µs | 1.996–2.083 ms |

The broader 5 s run is in [baseline-5s.csv](baseline-5s.csv). It included one
5,000/s row with p99 scheduled→done of 79.3 ms. A focused repeat taken while
two `clippy-driver` processes and a `librepaper` process were consuming CPU
produced p99 values of 9.8 ms, 169 ms and 57.7 ms. These observations are
consistent with sensitivity to host contention, but the machine was not a
controlled, dedicated benchmark host. Retain the full range rather than
treating the faster repeat as a guaranteed latency bound. The host load average
was about 3–7 during these runs.

The synthetic slow-probe run is in [induced-1ms.csv](induced-1ms.csv). It
replaces the query with `SELECT 1 FROM pg_sleep($1)` and should not be read as
a network measurement. The requested sleep was 1 ms; measured query service
time averaged about 2.07 ms. At 1,000 callers × 5/s (5,000/s offered), this
serialized probe sustained about 483/s, accumulated a 9.35 s drain tail, and
reached 9.25 s p99 scheduled→done. At 1,000 closed-loop callers, median mutex
wait was 1.53 s and p99 end-to-end was 2.08 s.

The evidence supports a narrow conclusion: the mutex does become the dominant
cost when verification service time is slow or when callers are driven in a
fully closed-loop burst. With local PostgreSQL and realistic paced editor
traffic, 5,000 verification probes/s sustained in the quiet repeat with about
2 ms p99 scheduled-to-completion. This is not evidence that the full server
supports 5,000 edits/s; it isolates only the lease verification boundary.

## Safety assessment (source inspection)

`Server::verify_writer` holds one deployment-wide Tokio mutex while
`WriterLease::verify` executes `SELECT 1` on the session holding the advisory
lock. It checks connection liveness, not the durable ownership epoch. Its only
production caller is the nonempty socket document-update path.

The actual durable-write fence is checked inside `begin_fenced_flush` and
`begin_document_command`: the transaction locks and compares the deployment
epoch, then locks the document. Once a new owner has advanced that epoch, the
old owner's transaction cannot persist its buffered edits. The normal flush
acknowledges newly saved updates after commit.

Even today, ownership can change between a successful probe and ingestion.
The old process can temporarily buffer and relay an edit before a later fence
check rejects its flush. Caching a successful probe would widen that window;
the separate transaction fence does not make changes to relay/reconnect behavior
automatically safe.

Existing storage tests cover takeover and rejected stale flushes. This
investigation did not add or run a live-socket takeover test. Before changing
verification frequency, test loss of the writer session, concurrent waiters,
takeover between probe and ingestion, absence of false durability signals, and
client reconciliation with the new owner. Keep idle deployments free of new
periodic probe queries.

There is also no explicit query timeout around the held writer connection in
this path. The pool's acquisition timeout does not bound a query on an already
acquired connection. A stalled probe can therefore block unrelated updates;
that failure behavior merits investigation separately from ordinary throughput.

Given these measurements, leave production behavior unchanged. Sharing a probe
among concurrent callers or a short lazy validity cache remain candidates for
measured slow-database deployments, not established large wins on a local VPS.
