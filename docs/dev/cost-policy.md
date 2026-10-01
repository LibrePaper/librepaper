# Operator cost policy

LibrePaper limits logical admission; it does not keep a billing-grade count of physical copies. Use provider metrics for actual database, disk, bucket, and backup use, and configure provider alerts and spending caps.

## Defaults

| Resource | Default | Set with |
|---|---|---|
| Document log | 32 MiB | `log_quota_mb` |
| Owner storage | 50 MiB | `--publisher-storage-limit` |
| Owner uploads | 30/hour | `--publisher-upload-limit` |
| Deployment storage | 5 GiB | `--deployment-storage-limit` |
| Memory | 512 MiB | `memory_budget_mb` |
| Pending source | 64 MiB | `pending_mb` |
| Write scratch | about 160 MiB | `pending_scratch_mb` |
| PostgreSQL connections | 20 | `--database-connections` |

Names beginning with `--` are serve flags; the others are advanced configuration keys.

Retained quotas count current document bases and rows, figures, and label archives; superseded bases are excluded. A requested archive counts once produced. Dereferencing a figure does not free it; deleting the document does. Identical re-uploaded bytes reuse the existing object. Admission is checked before writes and again at commit, so concurrent work can briefly create quota-exceeding or orphan bytes. Cleanup is asynchronous.

The log quota bounds build input, not build memory or time. Caches, builds, projections, request bodies, and transfers share the memory budget. POST, PUT, and PATCH require `Content-Length` and reserve four times that length. Pending scratch is about five times the largest framed row plus 32 KiB. Startup requires memory of at least fourteen times the log quota. See [resource limits](../host.md#storage) for failure behavior. Other request and socket ceilings are in [`cost.rs`](../crates/librepaper/src/server/cost.rs), [`config.rs`](../crates/librepaper/src/config.rs), and [`socket_budget.rs`](../crates/librepaper/src/server/socket_budget.rs).

## Physical storage and backups

Physical use can exceed logical quotas because of PostgreSQL indexes/WAL, retired bases, temporary or orphan objects, caches, system files, and backups. Compaction retains the old base for at least seven days; cleanup may lag or fail. A rough transient estimate is `snapshot_bytes × replacements_per_day × 7 × active_documents`, plus current data and backups. This is an illustration, not a workload measurement. Quota calculations are in [`repository.rs`](../crates/librepaper/src/storage/postgres/repository.rs); retention behavior is in [`collaboration.rs`](../crates/librepaper/src/storage/collaboration.rs) and [`maintenance.rs`](../crates/librepaper/src/storage/maintenance.rs).

`admin backup` dumps PostgreSQL and copies objects referenced by that database snapshot, including retired bases. It verifies lengths and available SHA-256 metadata; legacy objects may lack digests. Each backup directory is another physical copy. Backup policy configuration records intended frequency, retention, and destination; it does not schedule backups, prune directories, or encrypt them. Set retention, copy recovery points off-host, and test restores. See [hosting](../host.md).

Estimate monthly spend from these items:

- VPS hosting for the application and PostgreSQL.
- Off-host backup storage and any transfer charges for copies stored elsewhere.
- Planned primary object storage, including stored-file and read/write charges. Files on the VPS's local disk are included in VPS hosting, so do not count them again here.
- Annual domain renewal divided by 12.
- Other metered services, such as email or billable traffic, only when used and not already counted above.

Browser rendering and local or bring-your-own AI integrations do not require hosted inference. CPU and memory needs depend on concurrent server work, so size from measured use and current provider prices.

## Static delivery and host comparison (2026-09-28)

All browser assets (the four wasm modules and the LaTeX engines and bundles)
live on one dedicated public OVH S3 bucket, not in the server binary. They are
addressed by digest, pinned by `assets.lock`, and cached immutable, and the
bucket is append-only, so its size only grows and pruning is not implemented.
The embedded-font Typst WASM is 33.8 MB raw / 14.4 MB gzip; the publisher
stores gzip-compressed eligible files with explicit MIME and cache headers, so
measure the uploaded bucket for billable size. See
[asset mirror publishing](asset-mirrors.md).

R2 offers free egress and monthly allowances of 10 GB-month, one million Class A and ten million Class B operations; storage and operations above these cost extra ([R2 pricing](https://developers.cloudflare.com/r2/pricing/)).

## Planned document storage architecture

For about 1,000 users, the proposed setup is:

- **VPS:** application and PostgreSQL.
- **OVH Standard Multi-Zone:** private uploads and document snapshots; restricted credentials, with LibrePaper enforcing access permissions.
- **Backblaze B2:** independent encrypted Restic backups of consistent `admin backup` output. Deduplication shares unchanged data across recovery points ([Restic](https://restic.readthedocs.io/en/stable/040_backup.html)).
- **OVH S3-compatible Object Storage:** the dedicated public bucket (region `bhs`) that serves the wasm modules and the LaTeX release to browsers.

Prices checked 2026-09-28: OVH Multi-Zone costs about **CA$2.13 per 100 GiB/month**, versus CA$0.96 for One Zone, before tax. The extra CA$1.16 buys redundancy across three independent zones; requests, retrieval, and egress are free under Canadian terms ([pricing](https://www.ovhcloud.com/en-ca/public-cloud/prices/), [redundancy](https://docs.ovhcloud.com/en/guides/storage-and-backup/object-storage/s3-regions-comparison)). B2 costs **US$6.95/TB-month**, first 10 GB free, with free API calls and egress up to 3× average stored data; further egress normally costs US$0.01/GB ([pricing](https://www.backblaze.com/cloud-storage/pricing)).

This storage plan is not configured or migrated. Verify region availability, S3 integration, and restores first. Fully accommodating 1,000 owners at 50 MiB requires about 49 GiB logical capacity plus physical overhead, so raise the current 5 GiB deployment quota accordingly; benchmark server capacity separately.

OVH Canada's VPS is a comparison candidate; LibrePaper has not migrated. On 2026-09-28 listed starting prices were CA$6.20/month (VPS-1: 2 vCPU, 4 GB RAM, 40 GB), CA$11.64 (VPS-2: 4 vCPU, 8 GB, 75 GB), and CA$16.83 (VPS-3: 6 vCPU, 12 GB, 100 GB). These promotional or commitment-based prices are not guaranteed renewal quotes. Canadian VPS plans include unlimited traffic to worldwide users; caps apply to certain APAC server locations. Check the final quote, taxes, add-ons, and measured capacity ([OVH Canada VPS pricing](https://www.ovhcloud.com/en-ca/vps/)).
