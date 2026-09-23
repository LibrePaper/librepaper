# Operator cost policy

LibrePaper controls cost with bounded requests, retained-byte quotas, rate
limits, bounded PostgreSQL pools and worker batches, and provider billing
controls. PostgreSQL and the object provider remain the authoritative sources
for actual resource use; LibrePaper does not maintain a billing-grade physical
object ledger.

The initial application limits are:

**One document:**

| Resource | Default | Flag or key |
|---|---:|---|
| Inline source | 4 MiB (max 8) | `--document-size-limit` |
| Retained assets | 32 MiB | `--document-assets-limit` |
| Log (compaction base plus rows since) | 32 MiB | `log_quota_mb` in config file |

**One owner:**

| Resource | Default | Flag or key |
|---|---:|---|
| Retained storage | 100 MiB | `--publisher-storage-limit` |
| Asset uploads per hour | 30 | `--publisher-upload-limit` |

**Process:**

| Resource | Default | Flag or key |
|---|---:|---|
| Deployment storage | 5 GiB | `--deployment-storage-limit` |
| Memory budget for decoded documents | 512 MiB | `memory_budget_mb` in config file |
| Pending source buffers | 64 MiB each | `pending_mb`, `pending_scratch_mb` in config file |
| PostgreSQL connections | 20 | `--database-connections` |
| One update | 4 MiB | fixed |
| Request body memory | 256 MiB | fixed |
| Live sockets per network | 128 | fixed |
| Live sockets per principal | 64 | fixed |
| Live sockets per document | 256 (32 editors) | fixed |

Retained storage includes labels' source archives and all completed document
assets. An archive is produced on request, not written for every label, so it
is not counted until something has actually asked for one. Dereferencing an
asset does not restore quota; the asset remains until its document is
deleted. Reusing identical bytes later reuses the existing document asset and
does not issue another sequential object-store PUT.

Compaction, archiving and deletion run on an in-process bounded queue, not a
polled table. The two numbers that actually bound server-side cost per document are
the log quota above, which is what limits how long a cache rebuild can
occupy a thread, and the memory budget, which is shared by every resident
document's cache entries, in-flight builds and projection buffers on the
process. The memory budget must cover a cold build at the quota, fourteen times
the log bytes at the measured expansion factors, and the server refuses to start
otherwise. A document that exceeds either limit becomes unreadable until an editor
exports, trims and re-imports it, or an operator raises the log quota with
`log_quota_mb` or the memory budget with `memory_budget_mb` in the configuration
file; ingest and typing continue while it is unreadable. See
[self-managed hosting](hosting.md#resource-limits).

LibrePaper checks coarse owner and deployment thresholds before blob writes and
again in the PostgreSQL transaction that commits metadata. Concurrent writers
may create harmless orphan bytes when one loses a uniqueness or optimistic
concurrency race. Lifecycle rules or maintenance remove temporary and orphan
objects later. A provider-side billing alert and, where available, a hard
spending cap remain required because cached estimates and asynchronous cleanup
cannot exactly match an invoice.

`--database-connections` bounds each process's pool. One `admin serve` process
serves a deployment and runs its background work in-process. Keep enough server
connections for administration and backup. Document size, asset, storage and
upload limits are set by flags shown by `librepaper admin serve --help`. The log
quota, memory budget and pending budgets are set in the advanced configuration
file passed with `--config`.

Fetch `/api/status` over loopback for operational state, for example
`curl http://127.0.0.1:8080/api/status`. PostgreSQL
database size and object-bucket byte/request metrics should come from their
respective providers and alerts. Do not use object listings to reconstruct
permissions or current heads; those live in PostgreSQL.

Backups are outside primary storage admission. A filesystem backup copies only
keys referenced by one exported PostgreSQL snapshot and verifies their lengths
and SHA-256 digests. Remote S3-compatible storage normally relies on provider
versioning or replication unless the deployment's disaster model requires a
second-provider copy. See [self-managed hosting](hosting.md) for commands.
