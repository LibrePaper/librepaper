# Catalog v3 cost and performance measurements

Measured 2026-09-13 from the Catalog v3 implementation with bounded
collaboration-backlog admission. The reproducible fixture is
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
| Update append | 1 room | 0.87 ms | 8.87 ms | 8.89 ms |
| Update append | 20 rooms | 6.21 ms | 18.70 ms | 21.43 ms |
| Update append | 100 rooms | 13.59 ms | 28.24 ms | 54.35 ms |
| Room reconstruction | 20 rooms | 1.06 ms | 1.27 ms | 1.27 ms |
| Room reconstruction | 100 rooms | 0.37 ms | 1.04 ms | 1.20 ms |

| Product operation | Input | Elapsed |
|---|---:|---:|
| Create source version | 100 KiB inline | 28.2 ms |
| Create source version | 1 MiB inline | 17.0 ms |
| Create source version | 4 MiB inline | 29.3 ms |
| Create version | 1 asset reference | 17.9 ms |
| Create version | 100 asset references | 28.7 ms |
| Create version | 512 asset references | 65.6 ms |
| Publish and activate | 4,096 files | 203.7 ms |
| Publish and activate | 256 MiB | 538.3 ms |
| Read document page | 200 rows | 6.34 ms |
| Read annotation timeline | 500 rows | 1.66 ms |
| Claim jobs | 1 worker / 100 jobs | 6.9 ms |
| Claim jobs | 4 workers / 400 jobs | 13.3 ms |
| Claim jobs | 16 workers / 1,600 jobs | 22.1 ms |

The 10,000-document/500,000-version fixture occupied 301,810,191 PostgreSQL
bytes and loaded in 13.6 seconds. That is about 30.2 KiB per active document at
50 retained versions, including indexes and page overhead. Update acknowledgment
p95 at 100 active rooms was 28.24 ms, below both the 50 ms interactive target
and the 100 ms acknowledgment gate. The 256 MiB bundle measurement
includes hashing, filesystem transfer, and verification outside the final
PostgreSQL transaction.

The asset fixture creates each digest once and later versions reuse it. Asset
bodies are not copied into source archives: the archive with 512 references
was 23,996 bytes. A repeated edit to that project writes one small compressed
archive and performs no asset PUT.

The backup/restore check used 1,000 documents, 1,000 distinct zero-byte
immutable archives, PostgreSQL custom dump format, and the same local
filesystem. Creating and verifying the recovery point took 256 ms; restoring
it into an empty database and new object directory took 954 ms. The restored
database contained all 1,000 versions. Its dump was 130,846 bytes and its
manifest was 186,409 bytes. This fixture measures per-object traversal and
schema restoration; the 500,000-version database-size fixture above remains
the database scaling measurement.

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
bundle and asset downloads and must be modeled from traffic.

Application limits meter retained asset bytes, archives, bundles, upload
rates, version rates, document counts, and bounded request/concurrency sizes.
The PostgreSQL contract suite exercises refusal at exact limits. Operators must
also configure provider billing alerts or hard caps; the application cannot
enforce a provider account budget.

## Deployment checks

Before production release, repeat this fixture on the intended smallest VM and
S3-compatible provider and record object-operation and transfer measurements.
Provider alerts
or caps must be verified in that deployment. Those are deployment results and
cannot be demonstrated by this local repository run.
