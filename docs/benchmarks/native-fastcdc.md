# Native FastCDC history benchmark

Date: 2026-09-11. This follow-up replaces the exploratory JavaScript chunker
with the established Rust `fastcdc` implementation. It measures an in-memory
encoding pipeline, not a running librepaper server or browser.

## Scope

The questions are whether native chunking is inexpensive enough for a server
fallback, and whether actual FastCDC boundaries preserve the storage benefits
seen in the [earlier experiment](history-storage.md). No production storage
implementation or specification is changed by this experiment.

The corpus is the same 1,256 source snapshots from seven independent histories,
generated from repository commit `75701a2b6744e788d0b7f0409764acf0169ddc13`.
Only the 50-revision README history is a real editing trace; the other histories
are deterministic, stylized modifications of real source files. Inputs are
roughly 2–84 KB, not multi-megabyte documents or full projects.

## Measurement environment

- AMD Ryzen AI 7 PRO 450, eight physical cores / sixteen hardware threads.
- SHA-NI, AVX2, and AVX-512 available; frequency boost enabled.
- Linux 6.18.45; Rust 1.99.0-nightly (`11177f223`, 2026-08-02).
- Release build; encoding runs single-threaded. Desktop scheduling and clock
  variation remain possible; these are wall-clock service-time measurements,
  not isolated CPU-cycle measurements or a cloud-vCPU guarantee.

## Results

Actual FastCDC encoding is inexpensive on this corpus and host. For the larger
histories, the complete measured encoding step averages **73–230 microseconds
per file snapshot**. That includes full-file SHA-256, boundary finding, chunk
hashes, document-local deduplication lookups, compression of missing objects,
and binary recipe serialization. It does not include a durable checkpoint commit.

### Encoding CPU service time

Times below are **microseconds**, not milliseconds. Each CDC cell is mean / p95
over all per-checkpoint samples from five fresh-history timing passes. The
whole-file column is its mean. These are not percentiles of cumulative history
times. A separate warmup precedes each case/method.

| History | Whole zstd-3 mean | FastCDC 1 KiB mean / p95 | FastCDC 4 KiB mean / p95 |
| --- | ---: | ---: | ---: |
| Small Markdown | 12.0 | 8.7 / 10.9 | 12.6 / 14.7 |
| Small Typst | 9.7 | 7.2 / 8.8 | 10.2 / 12.4 |
| Small LaTeX | 14.2 | 10.5 / 14.9 | 17.3 / 19.4 |
| Large localized | 159.3 | 73.4 / 76.2 | 78.3 / 82.3 |
| Large scattered | 207.8 | 163.5 / 194.1 | 230.0 / 289.9 |
| Large front insertions | 153.7 | 75.4 / 78.9 | 73.2 / 76.1 |
| Actual Git history | 146.6 | 88.6 / 152.6 | 100.6 / 192.9 |

The first snapshot of each history has an empty document-local object store.
Across the larger cases, its mean is 192–386 microseconds with CDC. “Cold” here
means no reusable objects, **not** cold CPU caches or an uncached storage read.
Compressor initialization is included in whole-history time, but precedes the
first individual checkpoint timer. Final store destruction is excluded.

The separate diagnostic pass puts boundary finding at about 12–18 microseconds
for the larger files. Full-file and chunk hashing together take about 40–60
microseconds. Compression plus object lookup varies with reuse, about 5–154
microseconds. Diagnostic stages are not taken from the uninstrumented timing
passes and should not be added to manufacture an exact total.

Whole-file encoding is a genuine native baseline using the same reusable zstd
context. CDC can be faster over a history because reused chunks are not
recompressed. It is not always faster: the 4 KiB scattered-edit case is slower
than whole-file compression here.

Full reconstruction verification, including chunk and complete-file SHA checks,
takes about 108–204 microseconds per version for CDC in the larger cases in a
single diagnostic pass. This decoder does not cache decompressed chunks across
versions. It is not a benchmark of the proposed client-upload validation protocol.

### Retained source encoding bytes

Numbers are KiB, including unique compressed payloads **and actual serialized
compact recipes**, excluding common logical tree/event metadata, storage indexes,
filesystem allocation, and transient workspace. Each method is independent;
nothing is shared across document cases. “Thin” keeps revision 0, every twentieth
revision, and the final revision as one union (11 of 201, or 4 of 50).

| History | Whole full | CDC 1 KiB full | CDC 4 KiB full | Whole thin | CDC 1 KiB thin | CDC 4 KiB thin |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Small Markdown | 350.9 | 139.4 | 307.2 | 19.2 | 9.8 | 17.8 |
| Small Typst | 303.5 | 91.7 | 243.4 | 16.6 | 6.8 | 13.7 |
| Small LaTeX | 439.5 | 173.5 | 438.9 | 24.0 | 12.9 | 24.0 |
| Large localized | 4736.2 | 487.7 | 609.3 | 259.2 | 60.5 | 64.5 |
| Large scattered | 5438.6 | 2379.6 | 4479.3 | 297.4 | 433.3 | 362.7 |
| Large front insertions | 4743.3 | 432.0 | 245.4 | 259.5 | 57.4 | 41.6 |
| Actual Git history | 963.8 | 260.3 | 329.9 | 80.1 | 96.7 | 89.6 |

On the real Git history, CDC 1 KiB retains about **3.7 times fewer source-encoding
bytes** than native whole-file zstd-3 before thinning. After retaining only four
versions, whole-file storage is smaller than either chunking variant. This
confirms that retention policy can change the best encoding.

For the large localized history, the 1 KiB variant stores 139.3 KiB of compressed
payload but 348.4 KiB of recipes: compact metadata is still a material cost.
The front-insertion case similarly favors the 4 KiB encoding once recipes are
counted. These two targets do not establish an optimal production threshold.

The earlier CLI baseline adds a zstd frame checksum whereas this native bulk
API uses its defaults without that checksum; native whole-file objects are four
bytes smaller per unique file in this corpus. SHA-256 reconstruction checks
remain present in both experiments. Use the native baseline for these ratios;
do not attribute framing or metadata differences to FastCDC boundaries.

### Correctness and method

- FastCDC is pinned to `=5.0.0`, using `v2020`, normalization Level1, seed 0,
  minimum target/4, maximum target*8, and even parameters. SHA-256 uses
  `sha2 0.10.9`; `zstd 0.13.3` links zstd 1.5.7. Exact dependencies are locked.
- Inputs are loaded and checked against their frozen manifest SHA before timing.
  Each pass starts fresh document-local recipe/object maps. Unchanged file
  digests reuse their encoding; only missing chunk IDs are compressed.
- Recipes have a 48-byte header (magic/version/kind, full SHA, raw file length,
  reference count) plus 36 bytes per reference (raw SHA and raw length).
  The whole-file variant uses the same format with one reference. A production
  whole-file descriptor could be smaller; no metadata-optimality claim is made.
- The decoder reads the serialized recipe and compressed object map, not raw
  source copies. All 1,256 snapshots reconstruct in all three methods:
  **3,768 full-history reconstructions**, plus **210 thinned reconstructions**
  after unreachable objects are excluded from the in-memory map.
- Four unit tests cover empty/small/larger inputs, duplicate-file reuse, missing
  or corrupt objects, reordered recipes, thinning reachability, and quantiles.
  Formatting, tests, and Clippy with warnings denied pass.

## Capacity interpretation

For a measured mean encoding service time `t` seconds, an arrival rate of
`r` changed-file encodings per second needs approximately `r * t` continuously
busy workers for encoding alone. Keep substantial headroom; this calculation
does not model queueing, burst latency, or parallel scaling.

One checkpoint every 30 seconds for each of 1,000 independently changing
single-file documents is 33.3 jobs/second; for 10,000 it is 333.3. Multiple
collaborators on one document must not multiply encoding jobs. Larger files,
multiple changed files per checkpoint, cold snapshots, and storage latency
change the estimate.

Using the measured larger-history means, the 10,000-document illustration
corresponds to approximately **0.024–0.077 busy encoding workers** on this host,
before the excluded work. This is an arithmetic extrapolation, not a concurrent
load test. At these file sizes the measurements support keeping native server
encoding available; they do not justify adding a client-upload protocol solely
to avoid boundary-finding CPU.

Thirty seconds is an illustrative workload, not a guaranteed current cadence.
The current code uses 30 seconds of quiet and a 300-second maximum pending-edit
interval. Its defaults also include 10,000 checkpoints/hour deployment-wide,
300/hour per owner, and 200 resident rooms / 512 MiB of room memory. These limits
would need separate assessment before admitting thousands of distinct active
documents; a fast encoder does not remove them. The room sweeper already bounds
concurrent room ticks to four, which is not a dedicated CPU-encoding pool.

## Not measured

- Storage reads/writes, network requests, SQLite transactions, filesystem
  allocation, leases, garbage collection, or crash recovery.
- CRDT snapshot extraction, session persistence, collaboration fan-out, complete
  tree construction, or real concurrent clients.
- WASM download size, browser/mobile speed, client upload protocols, or server
  validation of hostile encodings.
- Multi-megabyte files, binaries, realistic project asset distributions, or
  performance on the intended deployment hardware.

Consequently, a native CPU result can justify a bounded server encoding pool;
it cannot establish that the application supports a particular user count.

## Artifacts and reproduction

The [results JSON](native-fastcdc/results.json), [Rust harness](native-fastcdc/src/main.rs),
[Cargo manifest](native-fastcdc/Cargo.toml), [lockfile](native-fastcdc/Cargo.lock),
and [corpus generator](native-fastcdc/generate-corpus.mjs) are retained in the
repository. The main agent corrected and reran the inexpensive agent's initial
harness before reporting these measurements. Superseded results are not included.

From the repository root, with Node, Git, Rust/Cargo, and a C toolchain available:

```sh
bench_dir=$(mktemp -d)
node docs/benchmarks/native-fastcdc/generate-corpus.mjs "$bench_dir" "$PWD"
cargo run --release --locked \
  --manifest-path docs/benchmarks/native-fastcdc/Cargo.toml \
  --target-dir "$bench_dir/target" -- \
  "$bench_dir/corpus.json" "$bench_dir/results.json"
```

The generator reads only the pinned commit and its README history, not current
working-tree source contents. It requires that Git history to be available.
Cargo downloads the locked dependencies when they are not already cached.
Generated corpus/build files are disposable; preserve the JSON result before
cleaning the temporary directory. The standalone benchmark does not build or
modify application code.
