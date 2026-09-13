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
| Retained versions per document | 1,000 |
| Asset uploads per owner per hour | 30 |
| Version creations per owner per hour | 30 |
| PostgreSQL connections per process | 20 |
| Jobs claimed per worker pass | 8 |

Retained storage includes source-version archives, all completed document
assets, and publication files. Dereferencing an asset does not restore quota;
the asset remains until its document is deleted. Reusing identical bytes in
later versions reuses the existing document asset and does not issue another
sequential object-store PUT.

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

Use `librepaper admin status` over loopback for operational state. PostgreSQL
database size and object-bucket byte/request metrics should come from their
respective providers and alerts. Do not use object listings to reconstruct
permissions or current heads; those live in PostgreSQL.

Backups are outside primary storage admission. A filesystem backup copies only
keys referenced by one exported PostgreSQL snapshot and verifies their lengths
and SHA-256 digests. Remote S3-compatible storage normally relies on provider
versioning or replication unless the deployment's disaster model requires a
second-provider copy. See [self-managed hosting](hosting.md) for commands.
