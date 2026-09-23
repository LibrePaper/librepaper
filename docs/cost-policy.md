# Operator cost policy

LibrePaper controls cost with bounded requests, retained-byte quotas, rate
limits, bounded PostgreSQL pools and worker batches, and provider billing
controls. PostgreSQL and the object provider remain the authoritative sources
for actual resource use; LibrePaper does not maintain a billing-grade physical
object ledger.

A deployment is bounded by five configured numbers. Everything else is
derived from them or gone.

**One document:**

| Resource | Default | Flag or key |
|---|---:|---|
| Log (compaction base plus rows since) | 32 MiB | `log_quota_mb` in config file |

A document's text is bounded only by what its log may hold. An update that
would exceed the log quota is refused with a retryable reason and the document
waits for compaction. The log quota is also the bound on one update and one row.

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
| Pending write scratch | five times the log quota | `pending_scratch_mb` in config file |
| PostgreSQL connections | 20 | `--database-connections` |
| Live sockets per network | 128 | fixed |
| Live sockets per principal | 64 | fixed |
| Live sockets per document | 256 (32 editors) | fixed |

Deployment storage counts the same three kinds as retained storage. The memory
budget is also what request bodies and outgoing state transfers reserve from.
A mutation must declare its content length; a request without one is refused with
411. Four times the content length is reserved from the budget before the body is
read; a request that cannot reserve is refused with a retryable 429.

An archive is produced on request, not written for every label, so it is not
counted until something has actually asked for one. Dereferencing a figure does
not restore quota; the figure remains until its document is deleted. Reusing
identical bytes later reuses the existing document asset and does not issue
another object-store PUT.

Compaction, archiving and deletion run on an in-process bounded queue, not a
polled table. The two numbers that actually bound server-side cost per document
are the log quota, which is what limits how long a cache rebuild can occupy a
thread, and the memory budget, which is shared by every resident document's cache
entries, in-flight builds, projection buffers, request bodies and state transfers.
The memory budget must cover a cold build at the quota, fourteen times the log
bytes at the measured expansion factors, and the server refuses to start otherwise.
A document that exceeds either limit becomes unreadable until its history is
trimmed from the settings page, or an editor exports, shrinks and re-imports it,
or an operator raises `log_quota_mb` or `memory_budget_mb`; ingest and typing
continue while it is unreadable. See [self-managed hosting](hosting.md#resource-limits).

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

Backups are outside primary storage admission. A filesystem backup copies only
keys referenced by one exported PostgreSQL snapshot and verifies their lengths
and SHA-256 digests. Remote S3-compatible storage normally relies on provider
versioning or replication unless the deployment's disaster model requires a
second-provider copy. See [self-managed hosting](hosting.md) for commands.
