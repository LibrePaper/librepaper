# Current-schema storage inventory

Run `psql -X -v ON_ERROR_STOP=1 -f inventory.sql "$LIBREPAPER_DATABASE_URL"`
against the deliberately identified current-schema database. The transaction
uses a repeatable-read read-only snapshot and expects migration 0005. Capture
the output together with the observation time, PostgreSQL version, database
identity, and whether object-store and WAL access were available. Do not
include credentials in captured output.

The report separates update-log source history, live and retired snapshots,
asset references, source archives attached to labels, proposal branch payloads,
PostgreSQL heap/index/TOAST relation sizes, and database total size. The
catalogue values for immutable object bytes are reference estimates from
stored metadata; they do not measure object-store allocation, provider
versioning, replication, filesystem block allocation, or unreferenced blobs.
Use a separately authorized object-store listing for those. Snapshot rows
whose legacy size metadata is NULL contribute to the unknown-size count and
not to the known-byte sum. WAL disk size is intentionally reported as unknown
unless an operator with the required PostgreSQL permission runs the optional
`pg_ls_waldir()` query noted in the SQL. `pg_stat_wal` write counters are not
disk usage.

Generated render/export outputs are request-time products and are not
catalogue-owned persistent blobs in this schema; proposal branches are
reported separately as stored generated-work payloads. The query does not
measure process memory, temporary files, backup destinations, or object-store
orphan inventory. The separate [recovery drill](../frugal-recovery/README.md)
checks the application's named-object backup set using a synthetic fixture.

For local filesystem objects and backup copies, `node filesystem.mjs PATH [PATH ...]`
reports logical file bytes and allocated file blocks separately. Pass disjoint
roots; symlinks are counted as skipped, never followed. Directory metadata and
filesystem snapshots are excluded.
