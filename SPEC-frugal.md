# Frugal scaling

Exploratory architecture notes, 2026-09-20. These are proposals for investigation,
not an approved implementation specification or measured capacity claims.

## Objective

Support thousands of concurrent documents and users while minimizing storage,
database queries, CPU and RAM on a small VPS. Make each additional open document
almost free until someone does meaningful work.

Keep the current foundations: browser-side compilation, batched update
persistence, content-addressed assets, and evictable CRDT state. Distinguish
stored documents, connected readers, and actively edited documents when discussing
capacity: their costs are very different.

## 1. Measure database work on established sockets

The authorization path in `crates/librepaper/src/server/socket.rs`,
`reauthorize_connection`, caches authorization for one second. Incoming messages,
including application pings, pass through it. The cache expires on demand; there
is no once-per-second authorization polling loop for room sockets. Receiving a
document broadcast does not itself trigger this check.

The browser sends application pings every 25 seconds. For 1,000 admitted idle
room sockets, that implies about 40 authorization refreshes per second on
average, with possible bursts when heartbeat timers align. Approaching 1,000
refreshes per second requires 1,000 sockets continuously sending messages.
Editing and presence traffic are event-driven, not periodic background work.

Each cache miss currently reconstructs an `IndexEntry`: one query for the
document, then queries for links, grants and owner/guest accounts. Signed-in
browser sessions add an account lookup. The resulting estimates are:

| Workload | Authorization refreshes/s | Authorization SQL queries/s |
| --- | --- | --- |
| 1,000 idle room sockets | About 40 | About 160–200 |
| 1,000 continuously sending room sockets | Up to about 1,000 | Up to about 4,000–5,000 |

These are workload calculations, not measured load or capacity claims. They
exclude connection setup, HTTP requests, sharing changes and other database
work. Bearer authentication can take a different path. The catalogue queries
have supporting indexes; neither their count nor the one-second cache establishes
that authorization is a bottleneck.

Another recurring cost is writer-lease verification. Before ingesting a completed
document update, the socket handler calls `Server::verify_writer`, which holds a
shared mutex while `WriterLease::verify` issues `SELECT 1` on its dedicated
connection. This adds a serialized database round trip per update, independent
of authorization cache hits. Measure its latency and contention alongside
authorization and persistence; preserve writer fencing in any optimization.

Initial investigation, 2026-09-20: on a 16-thread desktop with local PostgreSQL
17.11, an isolated probe benchmark sustained about 5,000 checks/s. Three focused
repeats had p99 mutex waits of 0.28–0.30 ms, though other runs under host contention
had much worse tails. A real WebSocket benchmark of 1,000 documents with one
writer and one observer each sustained approximately 5,000 updates/s in two
five-second runs; median relay latency was 27–29 ms and p99 was 65–70 ms.
Every update still performed its writer check. These are short, tiny-document
relay measurements without periodic flushing, not VPS capacity or durability
claims, and there was no probe-disabled comparison to isolate potential savings.

A synthetic slowdown yielding about 2.07 ms per probe reduced isolated throughput
to roughly 483 checks/s and caused queue growth. The shared connection is thus
a plausible slow-database risk, but the local results do not justify an urgent
redesign. The probe checks connection liveness; transactions separately fence
on the ownership epoch. Amortizing probes still requires tests of stale relay,
durability signals and reconnect behavior after takeover. See the
[results, limitations and reproducible harnesses](tools/writer-socket-bench/REPORT.md).

Leave the authorization architecture unchanged until measurements show a
material cost. If they do, first investigate a focused authorization lookup
instead of reconstructing the full catalogue entry, including guest display
information. Preserve permission, session and expiry checks while reducing
queries and materialization.

Only if that is insufficient, consider an in-memory authorization record with:

- Account/session and document permission generations.
- Immediate invalidation when sharing, ownership, sessions or accounts change.
- Local expiry checks for expiring links.
- Transactional authorization retained for durable semantic commands.

The single-writer architecture makes this more tractable. Correctness requires
covering every revocation path and specifying how changes made outside the owning
process become visible. A generation cache adds those obligations and is not
automatically a simplification.

## 2. Budget unsaved bytes across the deployment

`crates/librepaper/src/log/sequencer.rs` caps buffered updates at 4 MiB per
document. The decoded-document memory budget is separate; the inspected
pending-update insertion path does not appear to reserve against a global
pending-byte budget. Confirm the complete accounting before implementation.

Across 1,000 documents, those individual allowances permit about 4 GiB of pending
payloads before other memory. This is a worst-case allowance, not expected normal
usage. A database slowdown could push many documents toward it simultaneously.

Add a shared budget for pending and in-flight persistence bytes. Under pressure:

- Flush earlier.
- Evict disposable caches.
- Apply backpressure fairly across documents while clients retain unsynced work.

Account for ownership transitions and temporary copies during a flush so the
budget remains meaningful while writes are in flight. Preserve honest durability
acknowledgments and client recovery behavior.

## 3. Make flush scheduling concurrent but tightly bounded

Current flush triggers are five seconds of quiet, thirty seconds of accumulated
age, or 1 MiB of buffered updates.

For illustration, 1,000 continuously edited documents reaching the age trigger
imply roughly 33 flush transactions per second before other triggers. Transaction
counts alone do not establish whether persistence costs more or less than
authorization: measure execution time, waits and write costs as well.

At revision `67c20b4`, `Registry::housekeep` already bounds concurrent flushes
using half of the available pooled connections after reserving the writer
connection: nine flushes with the default pool of twenty. Each document also
serializes its own flushes. Do not add another worker pool. Measure whether this
bound leaves sufficient capacity for interactive operations during slow writes;
consider age or memory-pressure prioritization only if starvation is observed.

Cross-document transaction batching is a later option only if measurements
justify the extra failure coupling. PostgreSQL already supports sharing WAL
flush costs through group commit; application batching should not be assumed
necessary to obtain that benefit. Keep durability guarantees intact.

Reference: [PostgreSQL asynchronous commit and group commit documentation](https://www.postgresql.org/docs/17/wal-async-commit.html).

## 4. Let readers choose how live they need to be

Outside the current investigation, at the user's request.

Readers receive source-change notices and fetch a projection. A popular paper
being edited can cause repeated authorization, serialization, transfer and
browser rendering across many viewers.

Start with the existing machinery. `web/src/components/Reader.svelte` already
uses ETags, coalesces in-flight project requests, and has a refresh scheduler
that defers work while hidden. However, the source-change handler calls
`refreshCurrentProject` directly, bypassing that scheduler for the initial
request. Routing source-change notices through the scheduler is a small candidate
change; verify refresh frequency and catch-up behavior before adding new modes.

For a larger product simplification, consider one default reader policy: keep
the displayed page stable and show “New version available” until the reader
chooses to refresh. Editors retain immediate collaborative updates. Preserve
stale-source checks when readers place comments, and define how refreshing
interacts with selections and drafts. Add an automatic reviewer mode only if
there is a demonstrated need for another freshness policy.

Cache the serialized projection once per source digest and safe visibility
context, and coalesce simultaneous requests for it. Authorize before delivery.
Inspect existing projection caching to identify what work is already shared and
what serialization or response work remains per request.

This reduces repeated work without adding another service. It changes reader
freshness, so the behavior should be explicit and selected as a product choice.

## 5. Treat storage as a lifecycle problem

Content-addressed assets and on-demand source archives already help. Measure
physical storage, including old bases awaiting deletion, PostgreSQL indexes and
WAL, and backup copies, rather than relying only on charged document bytes.

Investigate:

- Incremental backups that reuse immutable blobs while preserving consistent,
  verifiable recovery points.
- Lazy creation of the five starter documents, copying one when first edited.
- Separate accounting for source history, assets and generated artifacts.
- Compaction thresholds informed by bytes rewritten and cold-open latency.

The [read-only local inventory](tools/frugal-storage-inventory/REPORT.md) found
five copies of the same 11,694-byte starter asset: global deduplication would
save 46,776 bytes for this account. Two of five starters had never been opened.
This confirms duplication, but one development account on an older database
schema cannot establish deployment-wide savings or starter adoption. No local
backup inventory was available. Obtain representative measurements before
adding shared-object ownership and garbage-collection complexity.

Defer history truncation. The compaction measurements recorded in
`SPEC-server-is-a-log.md` do not currently justify sacrificing old-client merging
or operation-level blame. Revisit only with representative concurrent workloads
and long-lived documents that demonstrate a material cost.

## 6. Scale out by document when one machine runs out

The preferred eventual shape is one owning server per document, with routing by
document ID. Keep sequencing, sockets, caches and persistence coordination
together at that owner.

This requires replacing the deployment-wide writer lease with ownership at an
appropriate shard boundary, including safe fencing and ownership transfer. It is
a real architectural change, not simply starting additional replicas.

For now, retain one Rust process, PostgreSQL and blob storage as the baseline.
Introduce distributed coordination only when measurements establish the need.

## First experiments and priorities

Compare these workloads:

1. 1,000 idle readers.
2. 1,000 editors spread across documents, including the demanding case of one
   active editor per document.
3. A crowded document within configured admission limits.
4. A mass reconnect after a server restart or network interruption.
5. A database slowdown while many documents accumulate unsaved updates.

The 1,000-socket cases require deliberate admission setup. Defaults include
128 sockets per network bucket, 64 per principal, 256 per document and 32 editors
per document; unsigned sockets share an anonymous principal bucket. A single-host
anonymous load generator will hit admission limits first. Use distributed
identities and source addresses, or explicitly report overridden limits, and
spread connections across documents as appropriate.

Record process RSS, authorization cache hits/misses and refresh latency, query
counts, database wait time, writer-lease verification and mutex wait time,
durable-save latency, replay CPU, bytes transferred, pending-write bytes and
refusals. Include PostgreSQL memory and physical storage in the deployment totals.
Report document
sizes, edit rates, client roles and configured limits alongside each result.

Use [pg_stat_statements](https://www.postgresql.org/docs/16/pgstatstatements.html)
to identify cumulative query cost. Extend the existing workload and benchmark
harnesses where practical. The existing typing-throughput release benchmark
drives the registry/sequencer directly and bypasses socket authorization and the
socket's writer verification. It cannot establish their costs. Add a workload
that exercises real WebSocket connections, and separate connection setup from
steady-state traffic in the results.

Reader refresh changes are excluded from the current investigation. A global
pending-write budget addresses resilience during database slowdowns,
rather than a demonstrated routine performance bottleneck. Authorization and
writer-lease checks need measurement before redesign. Prefer a focused
authorization lookup over generation caching if the measured cost warrants it.
Bounded flush concurrency is already implemented. Defer
cross-document batching, sharding and history truncation until evidence supports
their additional complexity.
