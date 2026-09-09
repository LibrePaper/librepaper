# Existing TeXlyre bundle packaging

Catalog inspection only. Payload size is the internally LZ4-compressed .data asset; virtual size is the sum of catalogued file lengths. Neither number measures browser peak memory. No assets were rebuilt or deployed.

Mirror manifest: `709b30becccd4a4c6197007914dcf0b2ed8e3ef7ea0257d0252785aec9b7b577`. Release: `00f88f94b211c4d5`.

| Bundle | Files | Payload MB | Virtual MB | Format MB | Generated log MB |
| --- | ---: | ---: | ---: | ---: | ---: |
| texlive-basic.js | 7185 | 92.79 | 156.87 | 15.28 | 0.30 |
| texlive-recommended.js | 13236 | 201.20 | 290.11 | 15.28 | 0.34 |
| texlive-extra.js | 23883 | 341.57 | 521.79 | 15.28 | 0.50 |

## Implications

The basic bundle includes formats and runtime files for multiple engines. Serving it whole makes a pdfLaTeX document download resources for other engines. The catalog quantifies an opportunity to split/precompute resources; it does not establish how small a working bundle can be. Generated logs are removable build residue, but removing them alone will not solve startup cost.

13236 paths occur in multiple bundle catalogs, out of 23883 unique paths. These are overlapping trees, not three disjoint additions. Identical paths do not prove byte-identical contents; coherent repackaging must resolve those versions rather than blindly deduplicate by name.

These payloads are already internally compressed. Describing the local benchmark as lacking HTTP compression should not be read as saying the TeX files inside its .data assets are uncompressed. Additional transport compression may help, but requires measurement.

LibrePaper's current deployment uses Workers Static Assets. That service has a 25 MiB per-file limit: the oversized artifacts below need splitting, smaller engine builds, or an object-storage/CDN hosting path. This is a deployment constraint, not a reason to run compilation on a server. [Cloudflare limits](https://developers.cloudflare.com/workers/platform/limits/).

- busytex.wasm: 32.51 MB.
- texlive-basic.data: 92.79 MB.
- texlive-recommended.data: 201.20 MB.
- texlive-extra.data: 341.57 MB.

Reproduce with `node latex/benchmark/candidates/packaging.mjs`. Full catalog summaries are in PACKAGING.json.
