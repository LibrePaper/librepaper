# Mixed authorization and reconnect benchmark

`crates/librepaper/tests/frugal_mixed_bench.rs` starts the production server
against a disposable PostgreSQL database. For each requested cohort (default
100 and 1,000), it creates one document per socket, connects one owner/editor
per document, and emits one JSON object per phase. Database setup/migration
happens before the phase snapshots; the benchmark never truncates existing
rows. It uses uniquely named accounts and documents, so repeated runs leave
additional disposable data behind.

## Prepare PostgreSQL separately

Use a database you can discard. `pg_stat_statements` must be preloaded by the
PostgreSQL server and enabled in that database. For a local PostgreSQL 17
instance, configure the server before starting/restarting it:

```conf
shared_preload_libraries = 'pg_stat_statements'
compute_query_id = on
```

Then prepare the database and extension before running the measurement:

```sql
CREATE DATABASE librepaper_frugal_bench;
\connect librepaper_frugal_bench
CREATE EXTENSION pg_stat_statements;
```

If the test role cannot read `pg_stat_statements`, grant it the needed
monitoring role (for example `pg_read_all_stats`) or run with a role that can.
The benchmark does not reset global statistics. It snapshots query counts and
cumulative execution time around each workload phase and reports deltas, so
other concurrent traffic in the same database contaminates the sample. Keep
this database quiet during a run. Missing extension/permissions produce empty
statement deltas; they do not invalidate the socket and HTTP measurements.

Run the focused release benchmark:

```sh
export LIBREPAPER_FRUGAL_DATABASE_URL='postgresql://postgres:password@127.0.0.1:55439/librepaper_frugal_bench'
FRUGAL_MIXED_SOCKETS=100,1000 \
FRUGAL_MIXED_SECONDS=60 \
cargo test -p librepaper --test frugal_mixed_bench --release \
  -- --ignored --nocapture --test-threads=1
```

Migrations run as part of setup unless `FRUGAL_MIXED_SKIP_MIGRATE=1` is set;
set that only if the disposable database was migrated separately from this
checkout. Socket counts can be run one at a time, for example
`FRUGAL_MIXED_SOCKETS=1000`. Keep `FRUGAL_MIXED_SECONDS` at 50 or more for the
ping phase to cross multiple browser heartbeat intervals; its default is 60.
The mixed editing phase uses the same duration in one-second rounds, then
waits (at most 60 seconds) for retained pending bytes to drain.

`FRUGAL_MIXED_DB_STALL_SECONDS=45` holds an `EXCLUSIVE` lock on the update
table during the mixed phase. The control connection is separate from the
application pool. Reads and writer probes remain available while up to nine
housekeeping flushes wait for the lock. Its timer releases independently of
the workload. HTTP reads include the browser client header and are preflighted before timing.
Reconnect accepts either initial state or persisted rows, as required by the
room protocol. Edit and HTTP failures fail the benchmark; reconnect admission
failures are counted as an observed overload outcome.

## Admission and workload details

The benchmark deliberately overrides otherwise limiting admission values:

| Setting | Default | Benchmark |
| --- | ---: | ---: |
| Deployment sockets | 4,096 | 4,096 |
| Network bucket sockets | 128 | 4,096 |
| Principal sockets | 64 | 4,096 |
| Per-document sockets | 256 | 4,096 |
| Per-document editors | 32 | 4,096 |
| Requests per principal per minute | 6,000 | 1,000,000 |
| HTTP work concurrency | 64 | 64 (default) |

The socket ceilings permit the 1,000-socket cohort from one loopback peer and
one shared test principal. The elevated request allowance avoids the benchmark
account's request meter ending the HTTP phase early. The production HTTP work
concurrency limit remains in effect. Per phase, JSON includes DB pool occupancy and cumulative transaction-begin wait telemetry.
Those snapshots do not measure every single-statement read wait.

The `ping_authorization` phase sends the application ping once every 25 seconds
to every established socket and waits for `pong`. This matches the browser
heartbeat interval, though all benchmark clients are synchronized, so it
creates a burst instead of the smoother average expected from clients with
different connection times. Authentication defaults to a signed browser session cookie. Set
`FRUGAL_MIXED_AUTH=bearer` for the device-token path. The selected mode is
recorded in the configuration event. The first idle ping is sent at 25 seconds,
so connection setup cannot supply its cached authorization.

The `frequent_authorization` phase sends application pings at five per second
for ten seconds. This isolates repeated authorization and its one-second cache
from editing, writer verification, and HTTP query costs. SQL execution time and
ping round-trip latency are reported separately.

The `mixed_edit_http_flush` phase sends one small Loro text update per socket
per second, and issues an authenticated project HTTP GET for each
document per round. HTTP requests have a concurrency bound of 32. It records edit-plus-ping
round-trip latency (which drains incoming error/durability frames), full HTTP
response-body latency and failures, then waits for pending writes to drain.
The edit round-trip is not a durable-save acknowledgement measurement.
Rounds are closed-loop: waiting for responses adds to the one-second interval.

The `mass_reconnect` phase closes the cohort and reconnects it in batches of
128. It records end-to-end connection plus room-open latency and counts failed
handshakes (including server rejection responses) without retrying. This is a
single-process restart approximation: the server remains live, and it does
not measure process startup, load-balancer behavior, or client retry backoff.

## Reading results

Each output line is JSON. `pg_delta` contains per-normalized-statement call
and cumulative execution-time deltas along with totals. The
`authorization_candidate_*` subset matches statements mentioning
`documents`, `share_links`, `grants`, or `accounts` in SELECT queries; it is a query-text
classification, not a guaranteed one-to-one authorization attribution.
`writer_check_*` counts normalized `SELECT $1` statements as a proxy for the
writer liveness check. Compare these only with other traffic held fixed.

`pg_stat_statements.total_exec_time` measures time spent executing SQL. It
does not include application authorization-cache mutex waits, the writer
mutex wait, pool acquisition, or network wait, and it must not be presented as
those end-to-end costs. There is no claim that this harness can separate those
waits. PostgreSQL statement timings aggregate calls across the phase rather
than providing per-request latency.

The output is an instrumented local workload, not a capacity claim. Host
contention, PostgreSQL configuration, driver pool settings, request sizes,
client timing and cache state all affect results. No authorization architecture
change should follow from this benchmark alone; use measured cost and repeat
under the target deployment conditions first.

A project GET is preflighted before any timed phase. The state-transfer endpoint
requires a transfer token and is deliberately not used as an ordinary read.
HTTP failures retain their status or error category in the output.
