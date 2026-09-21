# Frugal scaling

Focused capacity investigation, pruned 2026-09-21. Retain measurements and
concrete resource gaps; these are not measured capacity claims. Speculative
architecture changes have been removed from the work list.

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

## 2. Budget unsaved bytes across the deployment: implemented

The reading was right. A document's buffer was capped at 4 MiB and nothing
counted the sum, so 1,000 documents permitted roughly 4 GiB of pending
payloads, and a database slowdown is exactly what pushes many documents
towards that at once.

`crates/librepaper/src/log/pending.rs` is the bound. One `PendingBudget` per
registry, handed to every sequencer, with two pools:

- **retained** (`pending_bytes`, default 64 MiB): accepted update payloads
  held in document buffers, charged as payload plus framing, reserved by
  `Sequencer::ingest` *before* the update is accepted into the document.
- **scratch** (`pending_scratch_bytes`, default 64 MiB): the encoded row a
  write builds and both driver buffers, including capacity growth, reserved by
  `Sequencer::flush` and by the semantic-command path before the row exists.

They are separate pools because the failure to avoid is a full budget that
cannot be drained: one undifferentiated pool would let buffers fill it and
leave nothing to encode the row that would empty them. `ingest` spends only
the first, persistence only the second, and
`Configuration::validate_pending` refuses a configuration whose scratch
ceiling cannot hold one maximum-size row. Reservations are `Drop` types owned
by the allocation they paid for -- a buffered batch owns its own charge, so
retiring a committed prefix releases exactly that prefix and a failed or
cancelled write keeps buffered work and destroys its database connection before
releasing driver scratch. Successful writes shrink buffers before pool reuse.

Three copies the accounting would otherwise have had to cover are gone
instead: the flush's `Vec<Batch>` snapshot of every payload, the clone
`ingest` made of the incoming vector, and the second `frame::encode` a
command ran after committing just to measure its own row.

Of the three responses this section asked for, the implemented one is
back pressure, with recovery. Flushing earlier was not added: the existing
1 MiB / 5s / 30s triggers and the one-second housekeeping sweep already
recover within seconds, and a second trigger would have meant a second
scheduler. Cache eviction already happens, against the decoded budget, and is
deliberately not wired to this one -- pending bytes are not a cache and cannot
be evicted. Fairness is **not** claimed: admission is first-come, and the
tests demonstrate that every document drains once pressure clears rather than
any share guarantee.

A refused update now carries a stable reason code and the server's head
vector, and the browser resends from it on a backoff (1s doubling to 30s, one
outstanding retry) -- so an editor who stops typing after a refusal still has
that work saved, without another keystroke. Retries incorporate newer durable coverage and are
cancelled when that coverage includes all local work, avoiding duplicate uploads
after successful gap recovery. Shutdown and other mandatory flushes wait for
scratch instead of skipping buffered documents. Back-pressure refusals no longer
count towards the socket's consecutive-refusal close, which would otherwise
have answered a slow database with a reconnect storm. Durable save status is
unchanged: it is still computed from the server's durable version vector, so
a refused edit reads as unsaved until a row actually carries it.

`docs/resource-bounds.md` §2 states what is accounted for, what is covered by
another bound, and what is excluded. **Remaining measurement limitation:** the
defaults are reasoned from the existing per-document ceilings rather than
measured against a real slow-database deployment. The RSS effect of the bound
has not been measured; the tests assert ownership and accounting invariants
directly, which is what the first experiment list below should now be run
against.

## 3. Verify the existing flush concurrency bound

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

## 4. Measure physical storage and recovery costs

Content-addressed assets and on-demand source archives already help. Measure
physical storage, including old bases awaiting deletion, PostgreSQL indexes and
WAL, and backup copies, rather than relying only on charged document bytes.

Measure source history, assets, generated artifacts, retired bases and backup
copies separately on a current-schema deployment. Verify complete recovery
using the existing backup path and the encryption procedure to be established in the
[security backlog](docs/specs/SPEC-security.md#3-backups-plaintext-recovery-points-remain-an-exposure).
Do not build a new backup engine or change compaction before those measurements.

The [read-only local inventory](tools/frugal-storage-inventory/REPORT.md) found
five copies of the same 11,694-byte starter asset: global deduplication would
save 46,776 bytes for this account. Two of five starters had never been opened.
This confirms duplication, but one development account on an older database
schema cannot establish deployment-wide savings or starter adoption. No local
backup inventory was available. Obtain representative measurements before
adding shared-object ownership and garbage-collection complexity.

Keep full collaboration history and the existing single-writer deployment.
The recorded compaction costs and local storage sample do not justify history
truncation, global asset deduplication, lazy starter creation, or document
sharding. Those implementation proposals are removed from this work list.

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
socket's writer verification. It cannot establish their costs. Extend the
existing real-WebSocket harness to include sustained persistence and realistic
document sizes; separate connection setup from steady-state traffic in the results.

A global pending-write budget addresses resilience during database slowdowns,
rather than a demonstrated routine performance bottleneck. It is now
implemented (section 2); what these experiments still owe it is a measurement
of where its defaults should sit. Authorization and
writer-lease checks need measurement before redesign. Prefer a focused
authorization lookup over generation caching if the measured cost warrants it.
Bounded flush concurrency is already implemented. Keep the existing
authorization and writer-lease architecture; there is no implementation task
for generation caching, cross-document batching, or a second flush worker pool.
