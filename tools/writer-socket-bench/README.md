# WebSocket writer-probe investigation

The harness is [writer_socket_bench.rs](../../crates/librepaper/tests/writer_socket_bench.rs).
It starts a real Axum server and authenticated WebSocket clients, with one active
writer and one observer on each independent document. Every update passes through
the production authorization, writer verification, ingestion and relay paths.

This is a short relay benchmark, not a durability or full deployment benchmark.
It does not run the periodic housekeeping loop. Writers send small Loro text
updates in synchronized rounds, and wait for each round's relays before sending
the next round. Scheduled-to-relay measurements include late rounds; send-to-relay
measurements do not. This workload exposes bursts but is not an unrestricted
open-loop load generator. Connection setup is outside the measured window and
is limited to 16 concurrent document pairs (32 socket handshakes), below the
default 64 concurrent HTTP work slots. The initial unthrottled setup hit that
admission limit at 100 documents; that failed run is not a capacity measurement.

Network and principal socket limits are raised explicitly to admit the local
clients, which all use one benchmark account. The runtime has eight worker
threads. The configured catalogue pool is otherwise unchanged. Measurements
include ordinary authorization cache misses. The SQL counter is diagnostic:
it counts matching `SELECT 1`/normalized `SELECT $1` statements in the benchmark
database, rather than instrumenting only `WriterLease::verify`.

## Reproduction

Use a new disposable PostgreSQL database. The harness migrates it and adds
uniquely named benchmark rows; it never truncates. To collect SQL counts, preload
`pg_stat_statements` when starting PostgreSQL and create the extension in that
database before running the test.

```sh
docker run --detach --rm --name lp-writer-socket-investigation \
  -e POSTGRES_PASSWORD=writer-socket-bench -e POSTGRES_DB=writer_socket \
  -p 127.0.0.1:55435:5432 postgres:17-alpine \
  -c shared_preload_libraries=pg_stat_statements
docker exec lp-writer-socket-investigation pg_isready -U postgres -d writer_socket
# Wait until pg_isready succeeds before creating the extension or running tests.
docker exec lp-writer-socket-investigation psql -U postgres -d writer_socket \
  -c 'CREATE EXTENSION pg_stat_statements'
LIBREPAPER_BENCH_POSTGRES_URL=postgresql://postgres:writer-socket-bench@127.0.0.1:55435/writer_socket \
WRITER_SOCKET_BENCH_DOCUMENTS=1,10,100 \
WRITER_SOCKET_BENCH_RATE=5 \
WRITER_SOCKET_BENCH_SECONDS=5 \
WRITER_SOCKET_BENCH_REPS=2 \
cargo test -p librepaper --test writer_socket_bench --release --offline -- \
  --ignored --nocapture --test-threads=1
# After all repetitions finish:
docker stop lp-writer-socket-investigation
```

For this investigation the application source was frozen at commit
`3352432d5b1ef071f6a8146a27ec6fd92c26f645`, and the new harness was copied into
that checkout. Concurrent workspace edits had introduced SQLx query-cache and
migration incompatibilities; they were not changed to make this benchmark run.
The writer ownership module in the frozen checkout is byte-identical to the
one inspected at the start of the investigation:

`038c329e56fc9a65faeeb81330342282c8981ec030d2cebfb3050bae1a39a151`

The workspace advanced to `b3188b285ba960c8c3270e0b0de948ddf33a25a5` during the
run. Its probe and mutex code remained unchanged; broader workspace changes
were not included in the frozen benchmark.

The separate [probe microbenchmark](../writer-lease-bench/REPORT.md) measures
mutex wait and query time directly. Socket latency by itself does not identify
how much time belongs to writer verification. No production optimization or
probe-disabled comparison is included in this investigation.
