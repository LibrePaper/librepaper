# Source-history storage benchmark

Date: 2026-09-11. Source commit: `75701a2b6744e788d0b7f0409764acf0169ddc13`.
Compression: zstd 1.5.7. Three inexpensive agents prepared the whole-file,
chunking, and base-relative experiments; the main agent audited accounting,
corrected inconsistencies, executed the final chunking run, and measured packing.

## Findings

A subsequent [native FastCDC benchmark](native-fastcdc.md) measures the actual
Rust crate, compact serialized recipes, and in-process encoding CPU cost on the
same corpus. The JavaScript CDC results below remain exploratory; use the
follow-up for native performance conclusions.

No encoding wins every workload. One-hop base-relative compression is particularly
effective for dense, localized edits. Chunking has the smallest payload on the
actual Git history. Thinning can reverse advantages because an encoding may
continue retaining bases/chunks that sparse independent snapshots do not need.

Packing small objects is a separate, substantial opportunity on the measured
ext4 filesystem. Better compression alone does not remove per-file allocation
overhead. The current server was not benchmarked end to end.

The latest-PDF-only decision is reflected in the specifications independently of
these source-only measurements. No PDFs or other user data were deleted by this
benchmark, and no production compression implementation was changed.

## Corpus and methods

- Three real small inputs: `examples/regression-tables.md` (2,772 bytes),
  `examples/intervals.typ` (2,298 bytes), and `examples/standard-errors.tex`
  (4,052 bytes), each followed by 200 synthetic localized word edits.
- The real 61,131-byte README followed by 200 localized-edit, scattered-edit,
  or front-insertion revisions. A scattered revision replaces 12 word matches;
  a front insertion adds one short numbered sentence. The generator is deterministic.
  These are stylized compression workloads, not natural editing traces or
  compilable variants. Replacement tokens grow the source somewhat over time.
- Fifty actual README revisions from Git, oldest first. This is the only real
  editing trace in the corpus.
- Total: 1,256 source snapshots. Each case is a separate document identity;
  sharing across cases is prohibited.
- Whole-file zstd at levels 3 and 9, deduplicated by complete-file digest.
- Gear-style content-defined chunking prototype at 1 KiB and 4 KiB targets,
  minimum target/4 and maximum target*8, with independently zstd-3-compressed
  chunks. It uses a fixed SHA-derived Gear table; it is **not a production
  FastCDC implementation or a performance comparison of established chunkers**.
  These are two separate encodings, not two representations stored together.
- One immutable base per case, independently zstd-3-compressed. Each later version
  uses `zstd --patch-from` against that base, or standalone compression if smaller.
  Delta depth is one; reconstruction uses the decompressed stored base.
- Sensitivity variant: start a new independently compressed base every 50 versions.
  It still has no delta chains. Fifty was one experimental choice, not a tuned
  policy recommendation.

All full-history source reconstructions passed byte-length and SHA-256 checks.
Whole-file retained versions were also decoded after selection. Chunk recipes
were read back from stored JSON and their referenced chunks decoded. Thinned
chunk/base results model reference reachability; they do not simulate a crashing
production GC or physically remove every unreachable object before verification.
Packing checks reconstruct each indexed object and verify its hash.

## Compressed source payload

All numbers are KiB (1,024 bytes), including required bases, excluding recipe,
encoding, tree, and event metadata and filesystem overhead.

| History | Whole zstd-3 | Whole zstd-9 | CDC 1 KiB | CDC 4 KiB | Fixed base | Base every 50 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Small Markdown, 201 versions | 335.2 | 325.1 | 129.0 | 332.5 | 45.9 | 31.0 |
| Small Typst, 201 versions | 287.8 | 279.3 | 215.9 | 286.7 | 45.2 | 29.8 |
| Small LaTeX, 201 versions | 423.8 | 412.2 | 241.3 | 422.5 | 44.1 | 34.4 |
| Large localized, 201 versions | 4720.5 | 4476.1 | 344.3 | 932.6 | 66.8 | 139.5 |
| Large scattered, 201 versions | 5422.9 | 5106.3 | 2494.8 | 4782.8 | 1967.0 | 705.1 |
| Large front insertions, 201 versions | 4727.6 | 4486.1 | 255.6 | 426.0 | 64.3 | 140.3 |
| Actual Git history, 50 versions | 959.9 | 911.8 | 160.6 | 283.9 | 398.0 | 398.0 |

Fixed-base payload is about 71 times smaller than whole-file zstd-3 for the
localized large-file case, but only about 2.4 times smaller for the actual Git
trace. CDC at the 1 KiB target is about 6 times smaller than whole-file zstd-3
on that trace. These ratios describe these inputs, not expected deployment-wide
savings. Refreshing bases helps accumulated scattered changes but stores
unnecessary additional full bases for localized edits.

## Including prototype metadata

Each method adds its encoding records; CDC also adds complete ordered recipes.
All methods below use the same uncompressed logical tree/event record sizes,
including only retained records after thinning. The standalone whole-file agent
also measured compressed common records; those were normalized to raw common
records here to avoid comparing different metadata policies.

The JSON encodings are prototypes, not equal-sized optimized formats: CDC uses
hex digests and per-chunk sizes; fixed-base descriptors contain more bookkeeping
than the grouped variant. The payload table above isolates compression benefits.
Compact binary recipes, dictionary-compressed metadata, and metadata packing
could materially change the following totals. These are retained byte lengths,
**not allocated disk space**.

| History | Whole zstd-3 | CDC 1 KiB | CDC 4 KiB | Fixed base | Base every 50 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Small Markdown | 467.9 | 372.5 | 466.7 | 179.1 | 141.2 |
| Small Typst | 420.5 | 368.3 | 420.8 | 177.8 | 140.0 |
| Small LaTeX | 556.5 | 430.4 | 556.7 | 176.7 | 144.6 |
| Large localized | 4853.8 | 1431.7 | 1307.2 | 200.5 | 249.9 |
| Large scattered | 5556.2 | 3537.9 | 5159.7 | 2101.4 | 815.5 |
| Large front insertions | 4860.9 | 1347.6 | 800.5 | 198.7 | 250.7 |
| Actual Git history | 992.9 | 399.7 | 365.5 | 431.5 | 425.3 |

### After thinning

Keep revision 0, every twentieth revision, and the final revision: 11 of 201
or 4 of 50. Count only the retained event/tree metadata and reachable encoding
objects. Required bases remain charged even if their originating event is removed.
Totals include prototype metadata and remain in KiB.

| History | Whole zstd-3 | CDC 1 KiB | CDC 4 KiB | Fixed base | Base every 50 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Small Markdown | 25.5 | 21.6 | 25.5 | 11.0 | 15.2 |
| Small Typst | 22.9 | 20.5 | 22.9 | 10.7 | 14.0 |
| Small LaTeX | 30.4 | 24.7 | 30.4 | 11.3 | 17.5 |
| Large localized | 265.6 | 108.2 | 93.9 | 32.8 | 124.3 |
| Large scattered | 303.7 | 448.2 | 371.2 | 136.0 | 164.0 |
| Large front insertions | 265.9 | 103.9 | 68.5 | 32.6 | 124.5 |
| Actual Git history | 82.4 | 102.5 | 94.2 | 49.8 | 49.3 |

The 50-version actual-history case has only one base in either base strategy.
Its identical payload but different total illustrates why prototype metadata
differences must not be mistaken for a compression improvement.

## Physical allocation and packing

Measured file allocation uses `stat.blocks * 512` on ext4. Packing concatenates
the already encoded payloads with a fixed 64-byte index entry per object. There
is no additional compression, and every packed object was read back and verified.
This experiment excludes directory/inode overhead, database pages, temporary
repack space, and deletion/compaction costs.

| Objects | Loose allocated KiB | Indexed pack allocated KiB |
| --- | ---: | ---: |
| Localized history, fixed-base payload | 824 | 80 |
| Localized history, CDC 1 KiB payload | 1012 | 364 |
| Actual Git history, fixed-base payload | 504 | 404 |
| Actual Git history, CDC 1 KiB payload | 864 | 176 |
| 402 example tree/event records | 1608 | 116 |

The metadata row is an intentional one-record-per-file stress test. Existing
catalogue events live in SQLite, so it is not a claim about current metadata
allocation. The source payload rows use the actual compressed objects produced
by these experiments. Packing does not change logical identities, but production
pruning would need bounded repacking to reclaim holes.

## Implications

1. Keep whole-file compression as a simple baseline and fallback. Raising zstd
   from level 3 to 9 provides only modest additional savings on this corpus.
2. Investigate packing or compact database storage for tiny payloads/recipes
   before promising that compressed byte lengths equal disk consumption.
3. Keep chunking under consideration: it outperformed base-relative compression
   on the only real edit trace. Measure compact recipes with an established
   chunker before choosing final parameters.
4. One-hop base-relative compression deserves a bounded implementation experiment
   for dense text history. Choose whether to refresh a base using evidence;
   blindly refreshing every 50 revisions is not consistently beneficial.
5. Evaluate encoding choices after retention as well as before it. Sparse retained
   snapshots may justify re-encoding, but that adds compaction work and temporary
   space requirements that were not measured here.

There are no production throughput conclusions: subprocess startup, JavaScript
BigInt chunking, and concurrent runs distort timings. This corpus does not cover
multi-megabyte papers, whole-project manifests, binaries, CRDT state, object-store
request overhead, shared dictionaries trained across documents, or crash recovery.

## Artifacts and reproduction

Normalized byte results: [history-storage-results.json](history-storage-results.json).
Scripts and detailed results are under `/tmp/librepaper-history-bench.NfMghZ`:

```sh
node /tmp/librepaper-history-bench.NfMghZ/build-corpus.mjs
node /tmp/librepaper-history-bench.NfMghZ/whole/bench.mjs
node /tmp/librepaper-history-bench.NfMghZ/cdc/bench.mjs
node /tmp/librepaper-history-bench.NfMghZ/base/benchmark.mjs
node /tmp/librepaper-history-bench.NfMghZ/aggregate.mjs
node /tmp/librepaper-history-bench.NfMghZ/pack-common.mjs
node /tmp/librepaper-history-bench.NfMghZ/pack-payload.mjs
```

The corpus builder pins the source commit above and regenerates temporary inputs.
Node and zstd must be installed. Git/zstd subprocesses required execution outside
the restricted sandbox in this environment. Generated corpus, candidate files,
decoded checks, and packs are disposable; retain scripts and JSON reports.
