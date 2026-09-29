# Operator cost policy

LibrePaper controls admission and work with retained-byte quotas, request and
upload limits, bounded PostgreSQL pools, memory budgets and worker batches,
alongside provider billing controls. These are operating limits, not a
physical-cost ledger. PostgreSQL and the object provider remain the sources
for actual resource use; LibrePaper does not maintain a billing-grade count of
every physical copy.

**One document:**

| Resource | Default | Flag or key |
|---|---:|---|
| Log (compaction base plus rows since) | 32 MiB | `log_quota_mb` in config file |

A document's text is bounded only by what its log may hold. An update that
would exceed the log quota is refused with a retryable reason and the document
waits for compaction. A single update may be as large as the log quota; row
framing adds a small amount beyond the update payload.

**One owner:**

| Resource | Default | Flag or key |
|---|---:|---|
| Retained storage | 100 MiB | `--publisher-storage-limit` |
| Asset uploads per hour | 30 | `--publisher-upload-limit` |

Retained storage counts label archives, figures, and each document's editing log
(base plus rows). The log is checked coarsely at ingest from an in-memory figure
rebuilt at each admission and moved by every flush and compaction, and exactly in
the figure transaction under the document row lock; two of an owner's documents
flushing at once can overshoot by a buffer. An update refused for this reason
comes back retryable with the reason `storage_quota`, and the editor keeps the
edit unsaved.

**Process:**

| Resource | Default | Flag or key |
|---|---:|---|
| Deployment storage | 5 GiB | `--deployment-storage-limit` |
| Memory budget | 512 MiB | `memory_budget_mb` in config file |
| Pending source buffer | 64 MiB | `pending_mb` in config file |
| Pending write scratch | about 160 MiB | `pending_scratch_mb` in config file |
| PostgreSQL connections | 20 | `--database-connections` |
| Requests per principal per minute | 6,000 | internal cost policy |
| Concurrent ordinary HTTP work | 64 | internal cost policy |
| Concurrent control HTTP work | 16 | fixed |
| Concurrent artifact transfers | 64 | internal cost policy |
| Live sockets per deployment | 4,096 | fixed |
| Live sockets per network | 128 | fixed |
| Live sockets per principal | 64 | fixed |
| Live sockets per document | 256 (32 editors) | fixed |

Pending write scratch is computed as about five times the largest framed row,
plus 32 KiB of slack. With the 32 MiB default log quota it is about 160 MiB;
the row includes batch, peer and row framing in addition to the update payload.
The request rate applies per principal; anonymous traffic is keyed by network
address. Internal cost policy defaults are defined in
[`config.rs`](../crates/librepaper/src/config.rs); these fields are not exposed
as configuration-file keys. See also
[`cost.rs`](../crates/librepaper/src/server/cost.rs) and
[`socket_budget.rs`](../crates/librepaper/src/server/socket_budget.rs).

Deployment storage counts the same logical categories as retained storage:
figures, archives and current document history. Superseded snapshots are
excluded from quota totals. The memory budget is also what request bodies and
outgoing state transfers reserve from. POST, PUT and PATCH requests must
declare their content length; a request without one is refused with 411. Four
times that content length is reserved from the budget before the body is read;
a request that cannot reserve is refused with a retryable 429.

An archive is produced on request, not written for every label, so it is not
counted until something has actually asked for one. Dereferencing a figure does
not restore quota; the figure remains until its document is deleted. Reusing
identical bytes later reuses the existing document asset and does not issue
another object-store PUT.

Compaction, archive production and deletion run as bounded in-process
background work. The log quota limits the input size of a document build;
the memory budget is shared by resident document caches, in-flight builds,
projection buffers, request bodies and state transfers. Request rates,
concurrent HTTP work, transfers and socket limits also bound server load.
The memory budget must cover a cold build at the quota, fourteen times the log
bytes at the measured expansion factors, and the server refuses to start otherwise.
An update that would cross the log quota is refused and waits for compaction.
Failure to reserve build memory returns `busy` without marking the document
unreadable. A decode failure or a completed synchronous build exceeding the
10-second diagnostic threshold does mark it unreadable; that threshold does
not interrupt the build. Ingest and flush continue within their admission limits, while projections and
semantic commands fail. Recovery requires resetting or re-admitting the
sequencer after addressing the cause; a history trim may itself fail if it
needs the unreadable state. See [self-managed hosting](hosting.md#resource-limits).

LibrePaper checks coarse owner and deployment thresholds before blob writes and
again in the PostgreSQL transaction that commits metadata. Concurrent writers
may create harmless orphan bytes when one loses a uniqueness or optimistic
concurrency race. Lifecycle rules or maintenance remove temporary and orphan
objects later. A provider-side billing alert and, where available, a hard
spending cap remain required because cached estimates and asynchronous cleanup
cannot exactly match an invoice.

`--database-connections` bounds each process's pool. One `admin serve` process
serves a deployment and runs its background work in-process. Keep enough server
connections for administration and backup. Retained storage and upload limits
are set by flags shown by `librepaper admin serve --help`. The log quota,
memory budget and pending budgets are set in the advanced configuration file
passed with `--config`.

Fetch `/api/status` over loopback for operational state, for example
`curl http://127.0.0.1:8080/api/status`. PostgreSQL
database size and object-bucket byte/request metrics should come from their
respective providers and alerts. Do not use object listings to reconstruct
permissions or current heads; those live in PostgreSQL.

## What a person sees

A signed-in account's settings page shows retained storage against the quota and,
under it, storage by document in three segments (figures, versions, history) with
a "Trim history" button per document. Trimming compacts the document to a shallow
snapshot at its current state and discards the rows; the document keeps its
content, named versions survive because they are source archives, comments whose
passage can still be found are re-attached by text and otherwise shown detached,
every live editor must rejoin, and the command refuses while another editor is
connected. Nothing trims on a schedule.

Backups are outside primary storage admission. The backup command writes a
database dump and copies all object keys referenced by that database snapshot
to a local directory, whether the primary object store is a filesystem or
S3-compatible service. It verifies lengths and SHA-256 where metadata is
available; legacy rows can lack object digests. That snapshot can name current
and retired collaboration bases, so the backup can be larger than logical
quota usage. The command does not prune older backup directories; operators
must set and enforce retention, and copy recovery points off the primary host.
The backup policy fields in configuration describe intended frequency,
retention and destination; they do not schedule backups, prune directories or
encrypt backup contents.
See [self-managed hosting](hosting.md) for commands.

## Cost and physical storage notes (2026-09-28)

### Cost model

The primary recurring cost is a fixed VPS running the Rust application,
PostgreSQL and reverse proxy. Browser-side rendering and local-companion or
bring-your-own AI integrations do not create a mandatory hosted inference
bill. Server work still costs CPU and memory: collaboration state
materialization and compaction, request handling, and native export paths run
on the deployment. Measure concurrent work and resource pressure when sizing a
host; signup count alone is a poor predictor.

Estimate a deployment's monthly cost from the services and copies it actually
uses:

| Component | What drives it |
|---|---|
| VPS | Fixed instance price, with CPU and memory sized for concurrent server work |
| Off-host backups | Backup size × retained copies, plus transfer or storage charges |
| Optional object storage | Stored bytes and provider request classes; include lifecycle and versioning behavior |
| Domain | Renewal cost amortized over the month |
| Metered traffic or services | Only where the selected provider charges for them |

Thus a useful estimate is `VPS + off-host backups + optional object storage and requests + domain / 12 + metered services`. It is an estimate, not a quoted bill: use current provider prices and measured deployment usage. Count retained bytes, concurrent work, backup copies and request volume; do not substitute account or signup counts.

The application quota defaults remain 5 GiB deployment storage, 100 MiB per
owner, and 32 MiB per document log. They bound logical admission, not disk or
bucket consumption. The owner and deployment calculations count current bases,
uncompacted rows, figures and retained label archives; superseded bases are
excluded. Physical storage can also include those retired bases during their
seven-day grace period, PostgreSQL indexes and WAL, deleted or orphaned objects
waiting for cleanup, database and object-store backups, caches and operating
system files. See the quota calculations in
[`repository.rs`](../crates/librepaper/src/storage/postgres/repository.rs).

Compaction replaces a base while keeping the prior snapshot for at least
seven days. The maintenance worker may delete it after that deadline, but
cleanup can lag or fail, so this is a minimum grace period rather than a hard
physical-storage bound:
([`collaboration.rs`](../crates/librepaper/src/storage/collaboration.rs),
[`maintenance.rs`](../crates/librepaper/src/storage/maintenance.rs)). A
rough churn illustration is
`snapshot_bytes × replacements_per_day × 7 days × active_documents`; add
figures, current bases, uncompacted rows, and every backup copy separately.
This formula illustrates a possible transient cost, not a measurement of
LibrePaper's workload. A backup exports a consistent database snapshot and
copies every object it references, including retired snapshots. Each full
backup directory adds another physical copy, and the command currently has no
automatic retention pruning ([`backup.rs`](../crates/librepaper/src/storage/backup.rs)).
Set retention explicitly, keep recovery copies off the primary host, and
regularly test a restore.

### Static delivery findings and plan

The LaTeX and Typst browser assets now use Cloudflare Workers Static Assets.
These are static assets without a Worker script or `run_worker_first`, so
requests do not invoke Worker code and do not require R2. The current published
LaTeX mirror contains 17,669 files totaling 10,845,586,184 bytes. Its largest
file is the 21,694,350-byte LaTeXML WASM. Automatic Brotli for that file is
5,802,815 bytes, about 31.7% larger than the previous quality-11 Brotli
sidecar. This is a delivery tradeoff, not an application or compiler change.

The new Typst WASM is 24,113,298 bytes (23.00 MiB), below the 25 MiB per-file
limit with about 2.00 MiB headroom; its 17 external fonts total another
9,683,068 bytes. Automatic Brotli for the compiler is 7,676,256 bytes and
excludes those fonts. The previous embedded-font WASM was 33,796,012 bytes and
could not fit as one static asset. Build-time fetching verifies the pinned
WASM and manifest checksums. At runtime the browser verifies the manifest
checksum and each font's size and SHA-256 before loading it. PDF fixture output
remained byte-identical in the migration checks.

Cloudflare documents unlimited static requests and no static-asset storage fee
on its free plan, with limits of 20,000 files on Free and 100,000 on Paid;
both plans have a 25 MiB-per-file ceiling. Automatic compression is not
available for every MIME type ([Static Assets billing and limits](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/),
[platform limits](https://developers.cloudflare.com/workers/platform/limits/),
[compression behavior](https://developers.cloudflare.com/speed/optimization/content/compression/)).
The LaTeX mirror is nearing the file-count ceiling, so monitor its count and
size at every release and keep a 25 MiB release gate. Retain immutable
hash-addressed URLs and font checksums, preserve embedded fonts in the native
Typst renderer, and do not split bibliography or hyphenation data merely to
meet the browser asset limit. Chunking the WASM is a future fallback only if
the module exceeds the limit; it is not implemented. The smaller Markdown,
bibliography and citations WASM files remain on the main application origin
with precompressed variants; this migration did not move every renderer.

Cloudflare R2 remains an alternative for object delivery, with free monthly
allowances of 10 GB-month storage, one million Class A operations and ten
million Class B operations, then usage-based charges. Its no-egress-fee model
was not selected for these mirrors; static assets fit the current deployment
better ([R2 pricing](https://developers.cloudflare.com/r2/pricing/)).

OVH Canada's VPS range is a candidate for a future primary-host comparison,
not a migration already performed. On 2026-09-28 its listed starting prices
were CA$6.20/month for VPS-1 (2 vCPU, 4 GB RAM, 40 GB), CA$11.64 for VPS-2
(4 vCPU, 8 GB, 75 GB), and CA$16.83 for VPS-3 (6 vCPU, 12 GB, 100 GB).
These are promotional or commitment-based starting prices rather than
guaranteed renewal quotes; taxes, backups, domain costs and other extras may
apply. OVH describes unlimited traffic from a Canadian VPS to worldwide users,
with region-based exceptions in APAC, rather than Canada-only traffic. Compare
the final quote and measured resource needs before choosing a host
([OVH Canada VPS pricing](https://www.ovhcloud.com/en-ca/vps/)).

The LaTeX and Typst mirrors have been deployed and verified. The primary
application still needs a rebuild and redeployment to ship the new browser
loader; the deployment session did not confirm that runtime update. The current
plan is to retain the two static mirrors, verify hashes on every release, and
monitor the LaTeX file count against its limit. Continue measuring
physical database, object and backup use separately from quota counters; define
backup retention and test off-host restores. Measure concurrency before
upgrading the primary server. Revisit chunked WASM delivery only if a browser
asset crosses the static-file limit.
