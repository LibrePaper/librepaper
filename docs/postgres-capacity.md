# PostgreSQL capacity

REVIEW-BIG-IDEAS.md §2.1 left one thing unverified: "that 20 PostgreSQL
connections suffice under 64 concurrent HTTP work slots plus background work.
Measure this under load." [`docs/resource-bounds.md`](resource-bounds.md) §9
records the same question as an inference rather than a measurement.

This document answers it. It inventories what actually touches the database
and when, records what was measured and on what, names the first thing that
saturates, and says what changed because of it. Everything here was either
read at the cited line or produced by a run whose command is given.

The short answer is that the connection count was never the constraint, and
could not have been: the path that carries live editing was writing one
document's row at a time for the whole deployment, so it could not use more
than one connection no matter how many were configured.

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

No path holds a connection across CPU work, blob-store I/O or network
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
  `begun_with_no_connection_free` is what separates a queue from a slow
  server, which is why it is a separate counter and not a percentile.

## 4. The harness

`storage::postgres::benchmarks::active_document_capacity_benchmark`, beside
the existing §14.1 throughput benchmark and reusing its fixtures. Ignored by
default; it truncates every table in the database it is pointed at, so it is
only ever pointed at a disposable one.

It drives the production paths: `Registry::get` to admit a document,
`Sequencer::ingest` for each edit, a real `Sender` channel joined through
`Sequencer::join`, and `Registry::housekeep` on the same one-second tick
`server/serve.rs:540-550` runs in production. Durable latency is measured from
submission to the `doc-ack` frame arriving on that channel -- the frame an
editor's save indicator actually waits for -- rather than from a flush return
value nobody is sent.

It does not go through HTTP or a WebSocket, for the reason in §2.2: those
limiters are released before editing begins, so including them would measure
something the traffic never meets. Document-open and reconnect bursts *do* go
through them, and are a separate measurement this does not make (§8).

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

Each editor keeps a fixed cadence from a fixed start rather than sleeping
after each round trip, so a server that slows down does not quietly receive
less load. How far the generator fell behind its own schedule is reported as
`worst_editor_lateness_us`; attempted, accepted, retried and unacknowledged
work are counted separately from each other.

### Why the cadence is 8 seconds

`FLUSH_QUIET` is five seconds and `FLUSH_MAX_AGE` is thirty
(`log/sequencer.rs:72-75`). An editor typing without pause is flushed every
thirty seconds; one who pauses is flushed five seconds later. A cadence of
eight seconds puts every edit past the quiet deadline, so each edit produces
one row and the offered flush rate is exactly `documents / 8` per second --
the load can be reasoned about instead of inferred. It also sets the floor for
durable acknowledgement at five seconds, by design. What the measurement
watches is the distance *above* that floor.

## 5. What was measured

### Environment

Everything below is one laptop. **Do not read a production capacity claim out
of it.** What transfers is the shape of the curve and which resource runs out
first; the absolute numbers belong to this machine.

| | |
|---|---|
| Host | AMD Ryzen AI 7 PRO 450, 8 cores / 16 threads, 54 GiB RAM, NVMe |
| PostgreSQL | 16.15 in Docker (`postgres:16-alpine`), loopback, default configuration except `max_connections=300` and `shared_preload_libraries=pg_stat_statements` |
| Build | `cargo test --release` (a debug build measures Loro, not the server) |
| Commit | branch `capacity/postgres`, from `3352432d` |
| Workload | one editor per document, one edit every 8 s, 4 KiB starting documents, 60 s measurement, no compaction (thresholds put out of reach) |

The database and the application share the machine, so these numbers include
the server competing with its own database for CPU. A deployment with a
separate database host would see different absolute figures in both
directions.

### Baseline: the sweep writes one row at a time

| Active documents | Offered edits/s | Achieved rows/s | Durable ack p50 | p99 | Sweep pass max | Unacknowledged at end |
|---|---|---|---|---|---|---|
| 8 | 0.9 | 0.97 | 5.91 s | 5.97 s | 104 ms | 0 |
| 64 | 7.5 | 5.18 | 8.47 s | 21.6 s | 4.35 s | 0 |
| 256 | 29.9 | 31.0 | 6.33 s | 7.67 s | 1.82 s | 0 |
| 512 | 59.7 | 59.6 | 6.60 s | 15.6 s | 2.68 s | 0 |
| 1024 | 119.5 | **90.7** | 7.22 s | **32.9 s** | 4.01 s | **264** |

At 1024 documents the deployment stops keeping up: it is offered 119.5 edits a
second and makes 90.7 rows a second durable, 264 acknowledgements never arrive
inside the run, and the worst durable acknowledgement is 33 seconds against a
five-second design floor. A one-second housekeeping tick is taking up to four
seconds to complete.

And in every one of those runs:

```
"begun_with_no_connection_free": 0,
"begin_wait_max_us": 2533
```

**The pool was never contended, at any point on the curve.** Not once did a
transaction find the pool at its limit with nothing idle, and the worst wait
to open one was 2.5 ms. The saturating deployment was using about one
connection.

That is the answer to the question REVIEW-BIG-IDEAS.md asked, and it is not
the answer the question expected. The connections were not scarce because the
path that needed them could not ask for more than one at a time:
`Registry::housekeep` wrote one document's row, waited for its commit, and
then started the next. Each commit is a WAL fsync, which at 8 documents
measures about 13 ms a flush (a 104 ms pass for eight of them), so the whole
deployment had a ceiling of roughly 70 to 90 rows a second no matter how many
documents, editors, cores or connections it had. 90.7 is where it actually
stopped.

### Hot documents are cheap

| Workload | Offered edits/s | Achieved rows/s | Durable ack p99 |
|---|---|---|---|
| 64 documents, one editor each | 7.5 | 5.18 | 21.6 s |
| 64 documents + one shared by 16 editors | 9.3 | 7.88 | 6.28 s |
| 256 documents + one shared by 16 editors | 31.7 | 31.2 | 7.02 s |

Sixteen editors on one document cost about what one editor costs: their
batches coalesce into one buffer and leave as one row. The expensive
dimension is the number of *documents* being edited, not the number of people
editing. Sharing is the cheap case, which is worth knowing because it is the
opposite of the intuition that a busy document is the dangerous one.

## 6. The change

`Registry::housekeep` (`log/mod.rs:180`) now decides which documents are due,
then writes their rows concurrently, bounded by
`PostgresCatalog::flush_concurrency()` -- half the pool after the lease
connection is subtracted, so nine of twenty by default.

What did not change, and is what
`one_housekeeping_pass_flushes_every_due_document_exactly_once`
(`log/recovery.rs`) holds: concurrency is across documents only. One pass asks
each sequencer at most once, each flush still takes that document's own
transaction gate, so no document has two rows in flight and none is skipped.
The bound comes from the pool so a sweep cannot turn the queue into
`acquire_timeout` failures, and it is half rather than all of it so opening a
document or posting a comment does not queue behind a sweep.

### After

| Active documents | Offered edits/s | Achieved rows/s | Durable ack p50 | p99 | Sweep pass max | Unacknowledged |
|---|---|---|---|---|---|---|
| 8 | 0.9 | 0.97 | 5.94 s | 5.97 s | 60 ms | 0 |
| 64 | 7.5 | 7.76 | 5.90 s | 5.96 s | 81 ms | 0 |
| 256 | 29.9 | 31.0 | 6.00 s | 6.07 s | 109 ms | 0 |
| 512 | 59.7 | 62.1 | 6.01 s | 7.07 s | 1.17 s | 0 |
| 1024 | 119.5 | 124.1 | 6.27 s | 6.75 s | 833 ms | 0 |
| 2048 | 238.9 | 248.2 | 6.28 s | 6.85 s | 987 ms | 0 |
| 4096 | 477.9 | 482.0 | 6.64 s | 8.34 s | 3.15 s | 0 |

The saturation at 1024 is gone: offered and achieved match, nothing is left
unacknowledged, and durable acknowledgement sits just above the five-second
design floor plus a tick. The measured envelope moves from about 500
simultaneously active documents to at least 4096, at which point the
deployment is making 482 rows a second durable and using 193 MiB.

4096 is where the knee starts rather than where it fails: the sweep is back up
to 3.1 s in its worst pass and p99 acknowledgement has drifted to 8.3 s. The
next measurement to make is beyond it, and on real hardware.

### The pool is still not the lever

2048 active documents, same workload, only the connection count changed:

| Connections | Flush concurrency | Achieved rows/s | Durable ack p99 | Sweep pass max |
|---|---|---|---|---|
| 8 | 3 | 235.8 | 9.08 s | 3.42 s |
| 20 (default) | 9 | 248.2 | 6.85 s | 0.99 s |
| 60 | 29 | 248.2 | 6.54 s | 0.63 s |

Tripling the pool past the default buys nothing measurable in throughput and
a quarter of a second in the sweep's worst pass. Cutting it to 8 does cost
something. **20 is adequate and raising it is not the lever**; the default
stands, with the note that a deployment shrinking it below about 8 will feel
it on the flush path first.

### A hypothesis that did not survive

The `deployment_writer FOR SHARE` fence in §2.4 looked like a deployment-wide
cost: concurrent share locks on one row are what PostgreSQL allocates
MultiXactIds for, and every lock writes WAL against one page. It was measured
two ways rather than assumed.

A `pgbench` pair on the same server, identical except that one takes the
share lock and the other reads the same row without it:

| Clients | Fenced tps | Unfenced tps |
|---|---|---|
| 1 | 145 | 142 |
| 8 | 655 | 468 |
| 32 | 2,022 | 2,463 |
| 64 | 4,551 | 5,833 |

And in the application runs, MultiXactIds consumed went from 0 under the
serial sweep to 27,613 over 31,827 transactions at 4096 documents -- so the
allocation is real and the fence is what causes it. It did not stop throughput
rising more than fivefold across the same change. The share lock may cost
something in the region of twenty percent at high write concurrency; it is
not a wall, it is well behind the next real limit, and removing it would mean
re-arguing writer-takeover safety for no measured gain. **Left alone,
deliberately, with the counter now in the benchmark so a future run can
notice if that changes.**

## 7. Answers to the questions that were open

- **Do 20 connections suffice under 64 work slots plus background work?**
  Yes, with room to spare, and the comparison was the wrong one. Live editing
  is not under the work semaphore at all (§2.2), and the write path's
  concurrency was one, not 64.
- **Which traffic is covered by the HTTP work semaphore?** Requests and
  WebSocket handshakes. Not edits, flushes, acknowledgements or relay.
- **Is there deployment-wide serialization limiting independent documents?**
  There was: the sweep. It is gone. Two remain, below.
- **Should application replicas be considered?** Not on this evidence. The
  single-writer lease is doing its job, the bottleneck was a loop rather than
  a machine, and nothing here needs a second process.

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
3. **Total stored documents.** Every run here had at most a few thousand
   rows. Nothing was measured at a scale where index depth or planner choice
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
