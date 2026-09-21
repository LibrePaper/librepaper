# PostgreSQL capacity

REVIEW-BIG-IDEAS.md §2.1 left one thing unverified: "that 20 PostgreSQL
connections suffice under 64 concurrent HTTP work slots plus background work.
Measure this under load." [`docs/resource-bounds.md`](resource-bounds.md) §9
records the same question as an inference rather than a measurement.

This document inventories database consumers and describes an isolated editing
benchmark. It does **not** establish whether 20 connections suffice for 64
concurrent HTTP requests plus background work: those consumers are absent
from the benchmark. The mixed-workload question remains open.

The production change removes serial database writes from the housekeeping
sweep. Each document retains its transaction gate, and the sweep limits its
concurrent flushes to half the pool after allowing for the writer lease.

## 1. What "scale" means here

These are separate dimensions and they saturate in different places. Treating
them as one number ("how many users") is what makes a capacity claim
meaningless.

| Dimension | What bounds it today | Where |
|---|---|---|
| Total stored documents, including inactive | Nothing in the application; index depth and the background scan's paging | `document_log.rs:566` (`pending_background_work`), 64 rows a page |
| Simultaneously active (resident) documents | Nothing directly. Residency is cheap by design; the decoded half is bounded by the memory budget | `log/mod.rs:57-60`, `log/budget.rs` |
| Editors per active document | `document_editors_max` (32), and one sequencer lock per document | `server/socket_budget.rs`, `log/sequencer.rs` |
| Aggregate edit-operation rate | Per principal per document, 3,000/minute | `log/sequencer.rs:158-179` |
| Operation size | `MAX_UPDATE_BYTES` (4 MiB) per update, `FLUSH_TRIGGER_BYTES` (1 MiB) per row | `log/sequencer.rs:68,78` |
| Document size and accumulated history | `max_document` (4 MiB source), `log_quota_bytes` (64 MiB of log) | `config.rs:65-70` |
| Reconnection and initial-load bursts | The HTTP work semaphore (64) and the heavy-work semaphore (`min(cores, 4)`) | `server/cost.rs:62`, `log/admission.rs` |
| Background maintenance backlog | One worker task, 256-slot queue, 1024 deadlines | `storage/worker.rs:55`, `storage/schedule.rs:22` |

Two of these carry a hidden coupling that the rest of this document is about:
the number of simultaneously active documents and the aggregate edit rate both
run through one periodic sweep, and the background backlog runs through one
task.

## 2. Where the database is used

### 2.1 There is one pool

One `PgPool`, built once in `server/serve.rs:382-389` from
`--database-connections` (default 20, `storage/mod.rs:128-136`) with a
five-second `acquire_timeout` (`storage/postgres/mod.rs:88`). Nothing else in
the server opens a second one. `seed`, `backup` and the CLI build their own,
but they are separate processes.

One connection of the configured count never returns to it. `claim_writer`
(`storage/postgres/ownership.rs:73`) holds a `PoolConnection` for the life of
the process, because the deployment writer lease is a session-scoped advisory
lock and returning the session would release it. So a deployment configured
for 20 has 19 for everything else. This is correct and deliberate; it is
recorded here because a pool sized from the configured number is one short.

### 2.2 What the HTTP work semaphore covers, and what it does not

`server/cost.rs:240-248` takes a `work` permit (default 64, `try_acquire`, so
the 65th concurrent request is refused with `429` rather than queued) and
holds it across `next.run(request)`.

A WebSocket request passes through that middleware exactly once, for the
handshake. `server/socket.rs:291` hands the connection to
`WebSocketUpgrade::on_upgrade`, which returns the `101` response immediately
and runs the socket on its own task. **The permit is released at that point.**
Every edit, every flush, every durable acknowledgement and every relayed batch
for the life of that socket happens outside the work semaphore.

This matters for the original question. The comparison "20 connections against
64 work slots" describes the request path only. Live editing -- the traffic
this deployment exists for -- is not in either number.

### 2.3 Every consumer

| Path | Connection held for | Transaction | Statements | Concurrency |
|---|---|---|---|---|
| Authentication (`whoami`) | one statement | none | 1 | per request, under `work` |
| Document open over HTTP or socket handshake | several statements | none | ~6: `document_by_slug`, share links, grants, accounts (`document/store.rs:1265-1290`), then `document_by_slug` again inside `Rooms::get` (`room/mod.rs:420`), then `log_head` | per request, under `work` |
| Socket join | one statement, under the document's transaction gate | none | 1 (`log_coverage`, `log/sequencer.rs:1398`) | per socket, **not** under `work` |
| Edit relay (`ingest`) | none | none | 0 | in memory; no database at all |
| Edit durability (`flush`) | one transaction | fenced | 5: `BEGIN`, `deployment_writer FOR SHARE`, `documents FOR UPDATE`, `UPDATE documents`, `INSERT document_updates` (`document_log.rs:238-300,349-405`) | **one at a time for the whole deployment** before this change; see §5 |
| Semantic commands (comments, file operations) | one transaction | fenced and authorized | 5 plus the command's own rows (`document_log.rs:278-340`) | per request, under `work` |
| Compaction | one transaction, after the blob is written | fenced | activation plus row deletion (`document_log.rs:431`) | one at a time, one worker |
| Background scan | one statement at a time | none | 4 per page (`document_log.rs:566-625`) | one at a time, one worker |
| Deletion, archive, superseded-base sweep | one transaction each | fenced | varies | one at a time, one worker |
| Writer lease | the whole process lifetime | none | 1 per `verify` | one, permanent |

The flush path does not hold a connection across blob-store I/O or network
delivery. Compaction writes its snapshot to the blob store first and opens its
transaction afterwards (`storage/worker.rs:480-560`), and the Loro export runs
inside `spawn_blocking` under the heavy-work semaphore with no connection
held. `flush` snapshots the buffer under the in-memory lock, releases it, and
only then makes the round trip (`log/sequencer.rs:1201-1225`), so relay
continues while a row is written.

Pool exhaustion degrades rather than corrupts: `acquire_timeout` surfaces as
`sqlx::Error`, which becomes `WriteError::Storage`, which is a `503` with
`Retry::Later` and the message "storage temporarily unavailable"
(`room/error.rs:145-160`). A flush that fails this way leaves the batches in
the buffer, and ingest refuses retryably once the buffer reaches
`BUFFER_CEILING_BYTES` (4 MiB) with "sync_delayed: this server cannot save
work right now".

### 2.4 Deployment-wide serialization

Three things in the list above are deployment-wide rather than per document,
and they are the only candidates for making independent documents contend:

1. **The periodic sweep** (`log/mod.rs:180`), which is what flushes a document
   whose editor has paused. Before this change it wrote one row at a time.
2. **The background worker** (`storage/worker.rs:240`), one task, one job at a
   time: a compaction that takes seconds blocks every other document's
   compaction, every deletion and every archive behind it.
3. **The `deployment_writer` row**, which every fenced transaction locks
   `FOR SHARE` (`document_log.rs:246-252,286-292,449-455`,
   `ownership.rs:57-66`). Concurrent share locks on one row are what
   PostgreSQL allocates MultiXactIds for, and every lock writes WAL against
   one page.

The single-writer lease itself is not a candidate for removal. It is what
makes one sequencing boundary per document possible at all
(REVIEW-BIG-IDEAS.md §4), and nothing here proposes application replicas:
`SocketBudget` is documented as process-local
(`server/socket_budget.rs:88-90`), the memory budget is process-wide, and a
second process would take the advisory lock or refuse to start. Adding
replicas is a different change with a different safety argument.

## 3. Instrumentation

Three waits had to be told apart before anything could be tuned: the wait for
an application admission slot, the wait for a connection, and the work itself.
The first was already reported (`http_work` in the operator snapshot). The
second was not reported at all, so a deployment that was queueing for
connections and one that was running slow queries looked identical from
outside.

`storage/postgres/meter.rs` adds the second. Every transaction opens through
`PostgresCatalog::begin_metered`, which times the open and records whether the
pool had an idle connection at the moment it was asked. `GET /api/status` now
carries a `database` section:

```json
"database": {
  "max_connections": 20, "open_connections": 13, "idle_connections": 12,
  "checked_out": 1, "transactions_begun": 24,
  "transactions_failed_to_begin": 0, "begun_with_no_connection_free": 0,
  "begin_wait_mean_us": 287, "begin_wait_max_us": 620,
  "begin_wait_histogram": { "le_500us": 21, "le_1000us": 3, "...": 0 }
}
```

Counters and fixed buckets: no document id, account id, network or statement
text appears, so the cardinality is a constant.

Two limits, stated rather than implied:

- It covers transactions. Single-statement reads use the pool as an executor
  (`fetch_one(&self.pool)`) and acquire a connection inside sqlx, where there
  is no hook. `open_connections`, `idle_connections` and `checked_out` do see
  those, so pool saturation is visible; their individual waits are not.
- `begin_wait` includes the `BEGIN` round trip, not only the acquisition.
  `begun_with_no_connection_free` samples pool occupancy before acquisition;
  concurrent acquires and releases can race that sample. It is a contention
  signal, not proof that no caller ever waited for a connection.

## 4. The harness

`storage::postgres::benchmarks::active_document_capacity_benchmark`, beside
the existing §14.1 throughput benchmark and reusing its fixtures. Ignored by
default; it truncates every table in the database it is pointed at, so it is
only ever pointed at a disposable one.

It drives the production paths: `Registry::get` to admit a document,
`Sequencer::ingest` for each edit, a real `Sender` channel joined through
`Sequencer::join`, and `Registry::housekeep` on the same one-second tick
`server/serve.rs:540-550` runs in production. Durable latency is measured from
scheduled arrival to the `doc-ack` frame arriving on that channel -- the frame an
editor's save indicator actually waits for -- rather than from a flush return
value nobody is sent.

It does not go through HTTP or a WebSocket. The HTTP work permit is released
at upgrade, but real sockets still have admission limits, transport costs and
outbound queue budgets that this harness does not measure. Mixed HTTP traffic,
document-open and reconnect bursts require separate measurements (§8).

Knobs, all environment variables:

| Variable | Default | Dimension |
|---|---|---|
| `LIBREPAPER_CAPACITY_DOCUMENTS` | 64 | independent active documents, one editor each |
| `LIBREPAPER_CAPACITY_HOT_EDITORS` | 0 | editors sharing one additional document |
| `LIBREPAPER_CAPACITY_STORED` | 0 | stored but never-edited documents |
| `LIBREPAPER_CAPACITY_EDIT_MS` | 1000 | think time between edits |
| `LIBREPAPER_CAPACITY_SECONDS` | 60 | measurement window |
| `LIBREPAPER_CAPACITY_CONNECTIONS` | 20 | pool size |
| `LIBREPAPER_CAPACITY_SEED_BYTES` | 4096 | starting document size |

Each editor has an independent producer of scheduled arrival timestamps and a
64-entry queue. Producers never wait for ingest or retries. A full queue drops
an arrival and counts it in `arrival_queue_dropped`; scheduled demand is
computed independently, and `not_accepted` includes all scheduled arrivals
that did not become accepted edits. The consumer keeps the editor's causal
order, retries only until the workload deadline, and measures acknowledgement
latency from the scheduled arrival, including generator and queue delay.
`worst_editor_lateness_us` measures delay at consumption, including late timer
wakeups. Unaccepted arrivals are counted, not assigned successful latencies.

Opening rows are sampled before the workload and excluded from both reported
row counts. Offered and accepted rates use the configured workload duration.
The harness samples durable rows at the workload deadline and records the
actual sample completion time (the query itself can be delayed). Editors then
share a ten-second acknowledgement drain. A second row count, taken after the
sweeper finishes its current pass, uses the reported elapsed time including
drain and sweep shutdown. These rates have separate names and denominators;
neither includes setup rows. Pool counters remain lifetime counters, including
setup, as on the operator endpoint.

### Why the cadence is 8 seconds

`FLUSH_QUIET` is five seconds and `FLUSH_MAX_AGE` is thirty
(`log/sequencer.rs:72-75`). An editor typing without pause is flushed every
thirty seconds; one who pauses is flushed five seconds later. A cadence of
eight seconds puts edits past the quiet deadline when the sweep keeps up, so
each edit can produce one row. The long-run scheduled rate is `documents / 8`
per second; finite runs report their exact scheduled count divided by the
workload duration. If the sweep falls behind, multiple edits may share a row. It also sets the floor for
durable acknowledgement at five seconds, by design. What the measurement
watches is the distance *above* that floor.

## 5. Review of the original measurements

The original runs on a local PostgreSQL 16 laptop suggested that serial
housekeeping delayed durable acknowledgements and that concurrent flushing
helped. However, the original throughput tables are withdrawn:

- The numerator included one opening row per document written during setup.
- Reported rates included the final drain in the elapsed time, while tables
  compared them with offered load divided by the workload window alone.
- The generator waited for each ingest/retry before producing another edit,
  and latency excluded scheduling delay. Overload could reduce offered load
  or shift delay outside the latency distribution.

Consequently, the previous claims of 90 versus 482 rows/s, an envelope of
500 versus 4096 documents, and no benefit from increasing the pool are not
validated capacity results. The corrected harness needs a new release-mode
curve before numerical capacity claims are restored. The historical hot-document
and fence comparisons also do not establish full deployment capacity.

### Corrected accounting smoke check

A debug-build check with two documents, a ten-second workload and eight-second
edit cadence produced two scheduled and accepted edits (0.2 edits/s). Two
setup rows were excluded. No workload rows were durable at the workload
snapshot; both were durable after the shared ten-second drain, with no
unacknowledged edits. This checks accounting and drain behavior, not capacity.
Use the command in §9 with `LIBREPAPER_CAPACITY_DOCUMENTS=2` and
`LIBREPAPER_CAPACITY_SECONDS=10` to repeat this workload.

## 6. The change and its regression test

`Registry::housekeep` gathers due documents and flushes them with
`PostgresCatalog::flush_concurrency()`: half the configured pool after
subtracting the writer lease, with a minimum of one. At the default pool size
of twenty, the sweep runs at most nine flushes concurrently. Other consumers
share the same pool; this bound does not reserve connections for them or
prevent acquisition timeouts under mixed load.

`one_housekeeping_pass_flushes_every_due_document_exactly_once` holds the
writer fence with an exclusive row lock and observes blocked flushes through
`pg_stat_activity`. It requires exactly the configured concurrency to overlap,
checks that the blocked cohort stays bounded, then releases the fence and
verifies one row per document. A serial sweep fails the overlap check; an
unbounded sweep fails the upper-bound check. The empty-buffer assertion runs
before ingest, so slow database setup cannot make an edit unexpectedly due.

## 7. Conclusions and scope

- **Do 20 connections suffice for 64 HTTP work slots plus background work?**
  Still unverified. The editing benchmark omits both HTTP load and compaction.
- **Does the HTTP work semaphore cover socket edits?** No. It covers the
  handshake, and releases its permit at upgrade. This makes editing an
  additional source of work, not evidence that HTTP contention is impossible.
- **Does the sweep serialize independent document writes?** The new sweep
  allows bounded overlap; per-document transaction gates still enforce order.
- **Should the pool size or writer fence change?** This change provides no
  corrected capacity evidence requiring either change. Defaults and takeover
  protection are retained pending representative measurements.

## 8. What is still unmeasured

In rough order of how likely each is to be the next thing to bite:

1. **Background maintenance is one task** (`storage/worker.rs:240`). One
   compaction at a time for the whole deployment, and compaction takes
   seconds. A cohort of documents crossing the threshold together queues
   behind each other, and a document whose log passes `log_quota_bytes`
   refuses edits until its compaction runs. This has the same shape as the
   bug just fixed, and it is not measured here because this run puts the
   compaction thresholds out of reach on purpose. `compaction_cost_release_
   benchmark` measures one compaction; nobody has measured the queue.
2. **Document-open and reconnect bursts.** These *are* under the work
   semaphore and they do cost about six queries each, including a duplicated
   `document_by_slug` (once in `get_checked`, again inside `Rooms::get`). A
   reconnect storm after a restart is the workload most likely to make the
   pool matter, and it is the one this harness does not generate.
3. **Total stored documents.** The historical runs had at most a few thousand
   document rows. Nothing was measured at a scale where index depth or planner choice
   changes, and `LIBREPAPER_CAPACITY_STORED` exists for that run.
4. **A separate database host.** Every number here has the fsync, the
   application and the database on one laptop. Network latency to a real
   database would raise per-transaction cost and make concurrency matter
   *more*, not less.
5. **`documents_compaction_due` is a partial index with the thresholds
   written into it** (`0001_catalog.sql:78`), matching the clamped defaults.
   An operator who lowers `max_uncompacted_updates` below 100 makes the
   scan's predicate wider than the index's, and the background scan falls
   back to a sequential scan of every document. Not reached at default
   configuration; worth knowing before someone tunes that knob down.

## 9. Reproducing

```sh
docker run -d --rm --name lp-cap -e POSTGRES_PASSWORD=x -e POSTGRES_DB=librepaper \
  -p 55440:5432 postgres:16-alpine -c max_connections=300
export LIBREPAPER_BENCHMARK_POSTGRES_URL=postgres://postgres:x@127.0.0.1:55440/librepaper
LIBREPAPER_CAPACITY_DOCUMENTS=1024 LIBREPAPER_CAPACITY_EDIT_MS=8000 \
LIBREPAPER_CAPACITY_SECONDS=60 \
  cargo test --release -p librepaper --lib active_document_capacity_benchmark \
    -- --ignored --nocapture
```

It truncates every table in the database it is given. Never point it at
anything else.
