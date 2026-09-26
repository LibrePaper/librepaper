# Writer lease verification benchmark

This standalone benchmark measures the current writer verification shape without
linking the application: one dedicated PostgreSQL session is protected by one
`tokio::sync::Mutex`, and each operation executes the same `SELECT 1` used by
`WriterLease::verify`. It records mutex wait and query service time separately.

Run it against a disposable PostgreSQL database:

```sh
docker run --detach --rm --name writer-lease-bench-pg \
  -e POSTGRES_PASSWORD=writer-lease-bench -e POSTGRES_DB=lease_bench \
  -p 127.0.0.1:55433:5432 postgres:17-alpine
docker exec writer-lease-bench-pg pg_isready -U postgres -d lease_bench
# Wait until pg_isready succeeds before running the benchmark.
cargo run --release --manifest-path tools/writer-lease-bench/Cargo.toml -- \
  --url postgresql://postgres:writer-lease-bench@127.0.0.1:55433/lease_bench
# After all benchmark users of this disposable container have finished:
docker stop writer-lease-bench-pg
```

The run covers 1, 10, 100 and 1,000 concurrent callers, both closed-loop and
one request per caller every 1 s and every 200 ms (approximately one and five
updates per editor per second). It performs a 500 ms warmup and two 2 s
repetitions by default. `results.csv` and `results.csv.md` are written in the
current directory; set `WRITER_LEASE_BENCH_OUT` to place them elsewhere. The
CSV reports offered rate, observed rate over the measurement window (including
any drain tail), mutex wait, query service time, and scheduled-arrival-to-
completion latency.

For a focused repeat, use `--measure-ms 5000 --repetitions 3
--only-concurrency 1000 --only-pace-ms 200`. Run after compilation completes,
without other benchmark or build processes competing for CPU. Record host load;
these are probe measurements, not full-server capacity measurements.

For a focused repeat, use `--only-concurrency 1000 --only-pace-ms 200`.

`--server-delay-ms N` changes the query to `SELECT 1 FROM pg_sleep($1)`. This
is an induced server-side delay, useful for showing the serialized failure mode;
it is not a network latency measurement. The default zero-delay query is the
production query exactly.
