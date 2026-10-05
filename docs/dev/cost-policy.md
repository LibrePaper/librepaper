# Operator cost policy

- Admission limits control logical use; providers meter physical database,
  disk, object, and backup use. Use provider metrics and alerts for spend.

## Production limits

- The official deployment uses these limits. Publisher values and database
  connections are explicit in [`production.toml`](../../tools/deploy/production.toml);
  whole-deployment storage and resource ceilings use server defaults.

- **Per publisher:** 50 MiB across owned projects; 30 project creations, forks,
  or figure uploads per rolling hour.
- **Whole deployment:** 5 GiB logical storage.
- **Per document:** 32 MiB document log.
- **In-flight source:** 64 MiB deployment-wide; write scratch is derived from
  the log limit (about 160 MiB at the default).
- **Memory budget:** 512 MiB; startup rejects a budget too small for the log
  limit.
- **PostgreSQL connections:** 20.

- Self-hosters can set `[limits].publisher_storage_mib`,
  `publisher_uploads_per_hour`, `deployment_storage_mib`, `log_quota_mib`,
  `memory_budget_mib`, `pending_mib`, and `pending_scratch_mib`, plus
  `[storage].database_connections`. See the [TOML mapping](../../crates/librepaper/src/cli/server_config.rs),
  [configuration defaults](../../crates/librepaper-base/src/config/mod.rs), and
  [hosting](../host.md) for supported settings and behavior.

- Storage is charged to the project owner. Logical use includes figures,
  requested archives, the current compressed base, and uncompacted update rows
  with framing.
- Trimming history removes versions, archives, and unused figures; recent
  figures are retained. Deletion and orphan cleanup can lag, so physical
  storage may remain above the logical total.

- Upload allowance is per signed-in account across projects. A project
  creation, fork, or figure request counts once, regardless of its file count;
  editor keystrokes do not count.
- Accepted live edits reserve their bytes before relay. Figure, archive, and
  snapshot admission accounts for pending edits.

## Physical use and backups

- PostgreSQL indexes and WAL, retired bases, caches, temporary or orphan
  objects, and backups add physical use beyond logical quotas.
- `admin backup` dumps PostgreSQL and copies objects referenced by that
  snapshot. Each backup is another physical copy.
- The Docker kit's separate `resticprofile.toml` enables encrypted scheduled
  snapshots and retention when it contains a `[resticprofile]` table. The
  staging volume needs room for one full export;
  the repository needs room for retained snapshots. Test recovery. Versioned
  object stores may retain old pack versions after pruning; set lifecycle
  expiry for noncurrent versions.
- Production stores primary objects on the deployment's filesystem volume.
  Browser renderers and LaTeX assets use the separate public OVH mirror; see
  [asset mirrors](asset-mirrors.md).

- Estimate spend from the configured application host, PostgreSQL and object
  storage, backup destination, domain, and metered services. Count each
  service once and use current provider usage and rates; LibrePaper has no
  provider-independent price estimate.
