# Operator cost policy

LibrePaper controls cost with bounded requests, retained-byte quotas, rate
limits, bounded PostgreSQL pools and worker batches, and provider billing
controls. PostgreSQL and the object provider remain the authoritative sources
for actual resource use; LibrePaper does not maintain a billing-grade physical
object ledger.

The initial application limits are:

| Resource | Default |
|---|---:|
| Inline source per document | 4 MiB |
| Retained assets per document | 32 MiB |
| Retained storage per owner | 100 MiB |
| Deployment storage threshold | 5 GiB |
| One document's log (compaction base plus rows since) | 64 MiB |
| Process-wide memory budget for decoded documents | 512 MiB |
| Asset uploads per owner per hour | 30 |
| Label creations per owner per hour | 300 |
| Label creations per deployment per hour | 10,000 |
| PostgreSQL connections per process | 20 |
| One update | 4 MiB |

Retained storage includes labels' source archives and all completed document
assets. An archive is produced on request, not written for every label, so it
is not counted until something has actually asked for one. Dereferencing an
asset does not restore quota; the asset remains until its document is
deleted. Reusing identical bytes later reuses the existing document asset and
does not issue another sequential object-store PUT.

There is no jobs-claimed-per-pass figure any more. Compaction, archiving and
deletion run on an in-process bounded queue, not a polled table, so nothing
is claimed in a pass; see [SPEC-server-is-a-log.md](../SPEC-server-is-a-log.md)
§8.6. The two numbers that actually bound server-side cost per document are
the log quota above, which is what limits how long a cache rebuild can
occupy a thread, and the memory budget, which is shared by every resident
document's cache entries, in-flight builds and projection buffers on the
process. A document that exceeds either becomes unreadable until an editor
exports, trims and re-imports it, or an operator raises the limit; ingest and
typing continue while it is unreadable. See
[self-managed hosting](hosting.md#resource-limits).

LibrePaper checks coarse owner and deployment thresholds before blob writes and
again in the PostgreSQL transaction that commits metadata. Concurrent writers
may create harmless orphan bytes when one loses a uniqueness or optimistic
concurrency race. Lifecycle rules or maintenance remove temporary and orphan
objects later. A provider-side billing alert and, where available, a hard
spending cap remain required because cached estimates and asynchronous cleanup
cannot exactly match an invoice.

`--database-connections` bounds each process's pool. Multiply it by the number
of application and worker processes when sizing PostgreSQL. Keep enough server
connections for administration and backup. Upload, render, agent, room, socket,
and outbound-memory limits are also bounded by the server configuration shown
by `librepaper admin serve --help`.

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
