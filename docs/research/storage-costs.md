# Storage cost and compatibility review

Reviewed 2026-09-07. Target: 1,000 documents receiving edits continuously,
24 hours/day for 30 days. Three lower-cost agents investigated provider
compatibility, session data and pricing; the parent reviewed the code,
corrected their models and fixtures, and reran the local measurements.

The promising direction is a prototype that combines updates from many
documents into durable remote batches. Keep the existing save guarantee:
report work saved after the remote copy can be recovered. A live B2 test
remains worthwhile, but B2 is not a verified drop-in replacement. There is
no measured $50–100 total service budget or verified 1,000-room VPS size.

The subsequent design decision is in [persistence.md](../specs/persistence.md):
use an authoritative Turso catalogue and batched R2 edits, with remotely
committed saves. The calculator below remains an unbatched comparison
baseline; it does not predict the new shared journal's total bill.

## What the current code does

The current configuration waits for two seconds of quiet before saving and
five minutes of quiet before making a checkpoint. The proposed fifteen-second
floor and maximum dirty age are spec work, not existing configuration knobs.
See `crates/komodoc/src/config.rs` and `Room::tick` in
`crates/komodoc/src/room/mod.rs`. Five minutes of quiet does not establish twelve
checkpoints per editing hour or a five-minute host-loss recovery bound.

An ordinary persist writes a full Yjs state and can also update the current
bucket index when measured usage changes. A new checkpoint writes new text
blobs, a tree, a session snapshot and mutable metadata. The catalogue spec
moves the mutable metadata into SQL and eliminates ordinary persist updates
to document and total rows within a capacity reservation. Session journals
still write their backend's SQL rows. The calculator models the earlier
per-room snapshot design with SQL catalogue records, not today's complete
bucket write stream or the newly selected batched journal. Additional checkpoint session
PUTs must be supplied if the implementation cannot combine them with a
regular save. Unchanged blobs and identical checkpoints can be reused.

`rooms_max = 200` currently removes clean rooms with no sockets. It does not
reject the 201st busy room. A hard admission limit is still implementation
work. A room is a document, and several co-authors can share its save stream.

## Request costs, with assumptions visible

Run `node docs/research/storage-cost-model.mjs`, optionally with `--json`.
Use `--input /path/to/inputs.json` to override named workload inputs.
For example, `{"documents":1000,"checkpointRatesPerActiveHour":[4],
"changedBlobsPerCheckpoint":1}` supplies a different checkpoint scenario.

The formula for regular session PUTs is
`documents * days * active_hours_per_day * 3600 / save_seconds`.
Checkpoint frequency is an independent input, never inferred from quiet time.

| Save interval | Session PUTs/month | Session Class A cost | With 12 checkpoints/hour, 3 PUTs each |
| --- | ---: | ---: | ---: |
| 15 seconds | 172.8 million | $774.00 | $891.00 |
| 60 seconds | 43.2 million | $193.50 | $310.50 |
| 120 seconds | 21.6 million | $94.50 | $211.50 |

These are R2 Standard Class A charges for the combined workload, applying
the one-million free allowance once and rounding to billable million-request
units. Three checkpoint PUTs means two new text blobs plus one tree; it is
an assumption. Storage, GET/HEAD, LIST, retries, backups, extra snapshot
writes and other activity require separate inputs. R2 lists Standard Class A
at $4.50/million, Class B at $0.36/million, storage at $0.015/GB-month, and
free allowances of 1M Class A, 10M Class B and 10 GB-month.
[R2 pricing](https://developers.cloudflare.com/r2/pricing/)

The calculator keeps R2 object storage and Turso database size separate.
Its four 4,096-byte pages per checkpoint is an unmeasured sync scenario.
At twelve checkpoints/hour across this workload, that is 8.64M checkpoints,
17.28M logical checkpoint-row changes and 141.56 GB sync for one replica.
It is not measured Turso traffic; indexes, maintenance and retries can change
it. Logical affected rows are not substituted for provider-metered rows.

Turso Developer monthly billing is $5.99, including 9 GB storage, 25M rows
written, 2.5B read and 10 GB sync. Sync overage is $0.35/GB. The example
therefore has about $52.04 in base-plus-sync cost before any other charges.
Scaler is not automatically the cheapest plan: compare measured usage.
Free has 5 GB storage and 3 GB sync; neither provider promises a free service
merely because product payload is capped at 5 GB.
[Turso pricing](https://turso.tech/pricing?frequency=monthly)

## What batching can save

One shared batch every fifteen seconds produces 172,800 batch PUTs/month,
versus 172,800,000 separate room PUTs: exactly 1,000 times fewer requests in
this idealized case. The batch stream alone fits R2's free Class A allowance
only if other usage leaves that allowance available. This is not a total bill.

Bytes do not shrink by the same factor. More shards or a byte limit that
splits a batch adds requests. Full snapshots, history checkpoints, compaction,
reads, discovery metadata and retries also remain. Do not recreate one SQL
update per room per flush just to index the batches.

A bounded prototype needs to demonstrate:

- Acknowledged updates survive host loss, including discovery and replay of
  the last committed batch; a remote PUT with no recovery path is insufficient.
- Batch size and replay time stay bounded; slow or large rooms cannot delay
  every other room's save indefinitely.
- Restart, duplicate delivery and an ambiguous upload result recover safely.
- Deleting one document removes its data from shared segments through
  compaction or another explicit lifecycle, with quota release at reclamation.
- Full-snapshot, compaction and metadata costs are included in measurements.

Local disk can serve as a cache or write-ahead buffer. Marking an update saved
after only local fsync changes the guarantee even if remote upload happens
one minute later. Sending each room separately once a minute still costs
43.2M PUTs/month; a local buffer alone is not cross-room batching. A second
server also requires writer fencing and a tested takeover procedure.

Increasing the save delay is a product tradeoff, not a prerequisite for
batching. Batching could preserve a fifteen-second target while cutting
requests. The claimed 800x and 8x savings cannot simply be multiplied into
a total service-cost estimate.

## Backblaze compatibility

Backblaze explicitly lists S3 PUT, GET and LIST operations as free. This
can eliminate the dominant request fee; “provider choice is worth only 2x”
is not established. Storage and the chosen egress route still need pricing.
[B2 transaction pricing](https://www.backblaze.com/cloud-storage/transaction-pricing),
[B2 pricing](https://www.backblaze.com/cloud-storage/pricing)

There is a confirmed driver mismatch: `S3Store::delete` sends no `versionId`,
but B2 documents that such a request inserts a delete marker. It does not
physically remove the old versions. `BlobInfo.version` is an ETag, not a B2
version ID. Version-aware cleanup/accounting is required before the proposed
physical-reclamation contract holds.
[B2 delete API](https://www.backblaze.com/apidocs/s3-delete-object)

Conditional PUT support remains unknown. The official PUT header table
does not establish `If-Match` or `If-None-Match` semantics; omission does not
prove lack of support. The spec requires these protections even on one active
server. The current code's unconditional `--single-writer` bypass does not
validate that requirement. Configure the actual B2 region, not `auto`.
[B2 PUT API](https://www.backblaze.com/apidocs/s3-put-object)

No dedicated B2 test configuration was available, and no live bucket requests
were made. A safe test uses a private disposable bucket or an explicitly
isolated prefix, a restricted application key, and a fresh random prefix.
It must check wrong and current ETags, create-only writes, two clients racing
on the same ETag, immediate GET/list visibility, version enumeration and
physical cleanup of every test version. Exactly one conflicting writer must
win. Do not run the fixed `.komodoc-probe` against existing application data.

## Local measurements and review

`cargo test -p komodoc --lib tests::s3:: --offline` passed all eight selected
tests. They cover request construction, conditional responses and detection
of ignored conditions against a local fake bucket. They do not prove B2
compatibility or atomic races at a real provider.

The preserved [session harness](storage-workload/src/main.rs) uses the real
public `komodoc::session` APIs. Run it with
`cargo run --manifest-path docs/research/storage-workload/Cargo.toml --offline`.
The root reviewer reran the same source from the temporary measurement
project. Representative results, in bytes:

| Synthetic fixture | Visible text | Encoded Yjs state |
| --- | ---: | ---: |
| One small document | 27 | 131 |
| Four text files | 6,437 | 6,757 |
| 200 text files | 40,000 | 55,157 |
| One file after ten character replacements | 100,000 | 100,427 |
| Larger file after ten character replacements | 1,000,000 | 1,000,427 |

The harness also held 1,000 independent, pristine documents of about 100 KB
each simultaneously; their combined encoded state was about 99.87 MB. That
measures document objects, not a running 1,000-room server. No socket fan-out,
comments, rendering, remote I/O or long production edit trace was exercised.
The harness's 5.7 MB diagnostic exceeds normal admission and is labelled so.
Random IDs can cause small encoded-size variations between runs. Debug
encoding timings are diagnostic and do not establish production throughput.

Review rejected an initial 2x edited-state claim: the fixture had failed to
set the main file and accidentally created a duplicate. The corrected harness
asserts file count and visible content after each edit. Review also corrected
the calculator's conflation of session saves and checkpoints, and the use of
bucket storage size as database size. Allowance-boundary, rounding,
independent-stream and separate-store arithmetic checks passed.

The next experiment should replay a declared edit trace through real rooms,
with request counters and fault injection, to compare individual saves with
remote batches. Provider compatibility and full server capacity remain open
measurements, not conclusions from the microbenchmark.

## Review of further optimization suggestions

The most promising experiment combines delta persistence and cross-document
batching. They solve different costs: deltas reduce bytes, batching reduces
requests. Full snapshots can also be bundled usefully, and a delta log does
not make shared-batch recovery, indexing or erasure nearly free.

The reviewed harness now compares full-state encoding with vector-relative
updates on one valid 10,000-character document. It restores the base to a
fresh document, applies the delta, verifies the result, then applies the
delta again to verify idempotency. Representative byte counts:

| Edit | Full state | Delta |
| --- | ---: | ---: |
| Append one character | 10,118 | 26 |
| Delete one character | 10,129 | 14 |
| Replace all characters | 10,130 | 10,038 |

This supports small-edit byte savings, not a universal 10–100x CPU estimate.
Deletion alone left the state vector unchanged while still requiring a
durable delta: log dirty tracking cannot rely only on vector equality.

Other suggestions need these corrections:

- **Compression:** measure a representative corpus and CPU cost. Compressing
  bucket objects does not reduce the number of R2 PUTs or the pages replicated
  by a separate SQL catalogue. Framing can make tiny deltas larger. Readers,
  recovery, format/version detection, decompressed-size limits and quota
  accounting must agree; this is not a guaranteed format-free change.
- **Yjs garbage collection:** already enabled. Installed `yrs 0.27.4`
  sets `Options::default().skip_gc` to false and invokes GC during transaction
  commit. Komodoc uses that default. Some synchronization metadata remains;
  this is not a proof of a fixed relationship between live text and state
  size. Recreating documents to discard identities would need a separate
  offline-client compatibility design.
- **Durable digest cache:** potentially useful, but the current cold checkpoint
  repeats text PUTs rather than doing GET existence probes. SQL presence rows
  would add writes and must agree with upload completion, pruning, object
  reuse and repair. Incorrect positive cache entries can publish missing
  content. First measure cold-room PUT volume; a new per-blob SQL schema is
  not automatically the cheapest remedy.
- **Orphan audit:** the spec already advances an incremental cursor under
  budgets. Its 1,000-request hourly cap permits at most 720,000 bucket
  requests per 30-day month, before byte/time limits. The suggested 7.2M
  monthly LISTs assumes a different, full-hourly scan. The separate deletion
  queue has its own timer. A weekly audit also delays reclaiming capacity;
  choose a completion target and request budget together.
- **Geometric reservations:** potentially fewer growth transactions, but
  larger spare reservations consume other users' capacity. Rounding a 5 MiB
  requirement to 8 MiB strands 3 MiB per room: about 2.93 GiB over 1,000
  rooms. The present 64 KiB spare bound is deliberate. Known large uploads
  should reserve their whole requirement in one transaction, not loop through
  80 small transactions. Consider capped adaptation only if measured latency
  warrants it.
- **Local SQLite with replication:** a valid alternative to compare, not free
  durability. Default asynchronous shipping can lose acknowledged local
  writes if the host disappears before replication. Current Litestream offers
  `sync -wait` to await remote replication; integrating such confirmation
  changes application acknowledgement and failure handling. Shipping still
  transfers database changes and creates bucket requests, compaction outputs
  and snapshots. A local cache does not itself concede remote durability.
  [Litestream sync](https://litestream.io/reference/sync/),
  [configuration](https://litestream.io/reference/config/)
- **Awareness:** the cursor route broadcasts without marking session state
  dirty. The existing
  `awareness_reaches_the_other_editors_and_is_not_kept` test passed when rerun.
  It checks relay and non-retention; an explicit storage-write counter would
  strengthen the regression check.
- **Real editing patterns:** measure actual pause timing and checkpoint
  triggers. A 5–20x reduction from the sustained-load case is a hypothesis.
  Keep the 1,000-continuously-dirty-room scenario as an explicit capacity and
  abuse case even if ordinary traffic is lighter.

None of these findings establishes the quoted combined dollar savings or
one-day implementation effort. Measure traffic, prototype remote delta
batches with recovery, then prioritize remaining work by measured cost.
