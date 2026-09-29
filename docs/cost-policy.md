# Operator cost policy

LibrePaper limits logical admission; it does not keep a billing-grade count of physical copies. Use provider metrics for actual database, disk, bucket, and backup use, and configure provider alerts and spending caps.

## Defaults

| Resource | Default | Set with |
|---|---|---|
| Document log | 32 MiB | `log_quota_mb` |
| Owner storage | 100 MiB | `--publisher-storage-limit` |
| Owner uploads | 500/hour | `--publisher-upload-limit` |
| Deployment storage | 5 GiB | `--deployment-storage-limit` |
| Memory | 512 MiB | `memory_budget_mb` |
| Pending source | 64 MiB | `pending_mb` |
| Write scratch | about 160 MiB | `pending_scratch_mb` |
| PostgreSQL connections | 20 | `--database-connections` |

Names beginning with `--` are serve flags; the others are advanced configuration keys.

Retained quotas count current document bases and rows, figures, and label archives; superseded bases are excluded. A requested archive counts once produced. Dereferencing a figure does not free it; deleting the document does. Identical re-uploaded bytes reuse the existing object. Admission is checked before writes and again at commit, so concurrent work can briefly create quota-exceeding or orphan bytes. Cleanup is asynchronous.

The log quota bounds build input, not build memory or time. Caches, builds, projections, request bodies, and transfers share the memory budget. POST, PUT, and PATCH require `Content-Length` and reserve four times that length. Pending scratch is about five times the largest framed row plus 32 KiB. Startup requires memory of at least fourteen times the log quota. See [resource limits](hosting.md#resource-limits) for failure behavior. Other request and socket ceilings are in [`cost.rs`](../crates/librepaper/src/server/cost.rs), [`config.rs`](../crates/librepaper/src/config.rs), and [`socket_budget.rs`](../crates/librepaper/src/server/socket_budget.rs).

## Physical storage and backups

Physical use can exceed logical quotas because of PostgreSQL indexes/WAL, retired bases, temporary or orphan objects, caches, system files, and backups. Compaction retains the old base for at least seven days; cleanup may lag or fail. A rough transient estimate is `snapshot_bytes × replacements_per_day × 7 × active_documents`, plus current data and backups. This is an illustration, not a workload measurement. Quota calculations are in [`repository.rs`](../crates/librepaper/src/storage/postgres/repository.rs); retention behavior is in [`collaboration.rs`](../crates/librepaper/src/storage/collaboration.rs) and [`maintenance.rs`](../crates/librepaper/src/storage/maintenance.rs).

`admin backup` dumps PostgreSQL and copies objects referenced by that database snapshot, including retired bases. It verifies lengths and available SHA-256 metadata; legacy objects may lack digests. Each backup directory is another physical copy. Backup policy configuration records intended frequency, retention, and destination; it does not schedule backups, prune directories, or encrypt them. Set retention, copy recovery points off-host, and test restores. See [hosting](hosting.md).

Estimate monthly spend from these items:

- VPS hosting for the application and PostgreSQL.
- Off-host backup storage and any transfer charges for copies stored elsewhere.
- Planned primary object storage, including stored-file and read/write charges. Files on the VPS's local disk are included in VPS hosting, so do not count them again here.
- Annual domain renewal divided by 12.
- Other metered services, such as email or billable traffic, only when used and not already counted above.

Browser rendering and local or bring-your-own AI integrations do not require hosted inference. CPU and memory needs depend on concurrent server work, so size from measured use and current provider prices.

## Static delivery and host comparison (2026-09-28)

LaTeX and Typst browser assets use Cloudflare Workers Static Assets. Requests are free of Worker invocation charges when no Worker script or `run_worker_first` is configured; R2 is not used. Cloudflare documents unlimited static requests and no static asset storage fee on Free, with limits of 20,000 files on Free, 100,000 on Paid, and 25 MiB per file on both plans ([billing and limits](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/), [platform limits](https://developers.cloudflare.com/workers/platform/limits/)).

Measured asset sizes: the current LaTeX mirror has 17,669 files / 10,845,586,184 bytes; its largest file is 21,694,350 bytes. Typst compiler WASM is 24,113,298 bytes (23.00 MiB), plus 17 external fonts totaling 9,683,068 bytes. Automatic Brotli compiler size is 7,676,256 bytes and excludes fonts. The former embedded-font WASM was 33,796,012 bytes, over the file limit. Keep a 25 MiB release gate and watch LaTeX file count. Typst native rendering keeps embedded fonts; smaller Markdown, bibliography, and citations WASM remain on the main origin. Mirrors have been deployed; the main application redeploy is still pending as of this finding. Compression varies by MIME type ([compression behavior](https://developers.cloudflare.com/speed/optimization/content/compression/)).

LaTeXML automatic Brotli measured 5,802,815 bytes, 31.7% larger than its former quality-11 sidecar. Keep automatic compression and separate Typst fonts; consider WASM chunking only if the compiler exceeds 25 MiB.

R2 offers free egress and monthly allowances of 10 GB-month, one million Class A and ten million Class B operations; storage and operations above these cost extra ([R2 pricing](https://developers.cloudflare.com/r2/pricing/)).

## Planned document storage architecture

For about 1,000 users, the proposed setup is:

- **VPS:** application and PostgreSQL.
- **OVH Standard Multi-Zone:** private uploads and document snapshots; restricted credentials, with LibrePaper enforcing access permissions.
- **Backblaze B2:** independent encrypted Restic backups of consistent `admin backup` output. Deduplication shares unchanged data across recovery points ([Restic](https://restic.readthedocs.io/en/stable/040_backup.html)).
- **Cloudflare Static Assets:** existing browser mirrors.

Prices checked 2026-09-28: OVH Multi-Zone costs about **CA$2.13 per 100 GiB/month**, versus CA$0.96 for One Zone, before tax. The extra CA$1.16 buys redundancy across three independent zones; requests, retrieval, and egress are free under Canadian terms ([pricing](https://www.ovhcloud.com/en-ca/public-cloud/prices/), [redundancy](https://docs.ovhcloud.com/en/guides/storage-and-backup/object-storage/s3-regions-comparison)). B2 costs **US$6.95/TB-month**, first 10 GB free, with free API calls and egress up to 3× average stored data; further egress normally costs US$0.01/GB ([pricing](https://www.backblaze.com/cloud-storage/pricing)).

This storage plan is not configured or migrated. Verify region availability, S3 integration, and restores first. Fully accommodating 1,000 owners at 100 MiB requires about 98 GiB logical capacity plus physical overhead, so raise the current 5 GiB deployment quota accordingly; benchmark server capacity separately.

OVH Canada's VPS is a comparison candidate; LibrePaper has not migrated. On 2026-09-28 listed starting prices were CA$6.20/month (VPS-1: 2 vCPU, 4 GB RAM, 40 GB), CA$11.64 (VPS-2: 4 vCPU, 8 GB, 75 GB), and CA$16.83 (VPS-3: 6 vCPU, 12 GB, 100 GB). These promotional or commitment-based prices are not guaranteed renewal quotes. Canadian VPS plans include unlimited traffic to worldwide users; caps apply to certain APAC server locations. Check the final quote, taxes, add-ons, and measured capacity ([OVH Canada VPS pricing](https://www.ovhcloud.com/en-ca/vps/)).
