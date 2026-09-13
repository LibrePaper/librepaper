# Catalog v3 cost and performance measurements

Measured 2026-09-13 from revision `b3e029fa` plus the Catalog v3 implementation
in this worktree. The reproducible fixture is
`storage::postgres::benchmarks::catalog_v3_release_benchmark`; its machine
readable result is [catalog-v3-measurements-20260913.json](catalog-v3-measurements-20260913.json).

## Reference deployment

- release build on Linux, AMD Ryzen AI 7 PRO 450, 8 cores/16 threads, 54 GiB RAM;
- PostgreSQL 17.11 in Docker, `fsync=on`, `synchronous_commit=on`, 128 MiB shared
  buffers, 4 MiB work memory, and a 100-connection server limit;
- application pool: 32 connections for the concurrency benchmark;
- local ext filesystem blob store;
- 30 append samples per room count, with all rooms active at once. Times include
  pool acquisition and transaction commit.

The host has more CPU and memory than the recommended small server. This is a
regression baseline and transaction-shape check, rather than a promise of
identical latency on a low-end shared VM.

## Results

| Operation | Load | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| Update append | 1 room | 0.66 ms | 1.16 ms | 1.75 ms |
| Update append | 20 rooms | 1.44 ms | 3.05 ms | 12.52 ms |
| Update append | 100 rooms | 2.97 ms | 4.79 ms | 7.10 ms |
| Room reconstruction | 20 rooms | 1.12 ms | 1.50 ms | 1.50 ms |
| Room reconstruction | 100 rooms | 0.37 ms | 1.19 ms | 1.39 ms |

| Product operation | Input | Elapsed |
|---|---:|---:|
| Create source version | 100 KiB inline | 14.0 ms |
| Create source version | 1 MiB inline | 18.0 ms |
| Create source version | 4 MiB inline | 34.8 ms |
| Create version | 1 asset reference | 20.9 ms |
| Create version | 100 asset references | 27.9 ms |
| Create version | 512 asset references | 50.5 ms |
| Claim jobs | 1 worker / 100 jobs | 5.0 ms |
| Claim jobs | 4 workers / 400 jobs | 9.9 ms |
| Claim jobs | 16 workers / 1,600 jobs | 17.9 ms |

The 10,000-document/500,000-version fixture occupied 301,495,987 PostgreSQL
bytes and loaded in 13.6 seconds. That is about 30.2 KiB per active document at
50 retained versions, including indexes and page overhead. Every measured
ordinary product operation stayed below the 250 ms target. Update
acknowledgment p95 at 100 active rooms was 4.79 ms against the 100 ms target.

The asset fixture creates each digest once and later versions reuse it. Asset
bodies are not copied into source archives: the archive with 512 references
was 23,996 bytes. A repeated edit to that project writes one small compressed
archive and performs no asset PUT.

## Capacity and cost model

Costs depend on the operator's provider, so the model exposes billable units.
Let `C` be the monthly server/database price, `Ppg` the price per GiB-month of
PostgreSQL storage, `Pobj` the price per GiB-month of object storage, `Pput` the
price per million object writes, and `Pout` the price per GiB transferred. For
50 retained versions per document, 2 MiB of unique retained assets per
document, and one 20 KiB source archive per version:

| Active documents | PostgreSQL | Object bytes | Initial object writes |
|---:|---:|---:|---:|
| 100 | 2.9 MiB | 0.29 GiB | about 5,100 plus unique assets |
| 10,000 | 2.81 GiB | 29.3 GiB | about 510,000 plus unique assets |
| 100,000 | 28.1 GiB | 293 GiB | about 5.1 million plus unique assets |

The monthly estimate is `C + PostgreSQL_GiB*Ppg + Object_GiB*Pobj +
writes_million*Pput + egress_GiB*Pout`. At the 100-document tier PostgreSQL and
LibrePaper can share one machine and filesystem objects can share its disk, so
`C` can be the only fixed service charge. Larger deployments can move the same
schema to managed PostgreSQL and S3-compatible storage without an application
migration.

Rendering and attached agent execution run in the browser or the user's local
runner and do not consume server compute by default. Transfer grows with actual
publication and asset downloads and must be modeled from traffic.

Application limits meter retained asset bytes, archives, publications, upload
rates, version rates, document counts, and bounded request/concurrency sizes.
The PostgreSQL contract suite exercises refusal at exact limits. Operators must
also configure provider billing alerts or hard caps; the application cannot
enforce a provider account budget.

## Deployment checks

Before production release, repeat this fixture on the intended smallest VM and
S3-compatible provider and record publication-at-limit, timeline/listing,
backup, restore, object-operation, and transfer measurements. Provider alerts
or caps must be verified in that deployment. Those are deployment results and
cannot be demonstrated by this local repository run.
