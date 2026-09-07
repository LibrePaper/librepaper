# Hybrid result: fast prose edits, slow Biber updates

The hybrid passed the four-phase bibliography fixture: direct WASM
XeTeX/dvipdfmx typesets the document, while CheerpX runs native Biber. All
four PDFs match their respective native references in normalized extracted
text and page count. Every BBL is byte-identical to its native reference.
Unicode, the newly cited entry, and the changed bibliography title are present.

## Measured run

The final run used a fresh Chromium profile and retained the same engines
across these cumulative edits. These are single observations on this
machine, with local package delivery and no deliberate throttling.

| Phase | Complete phase | TeX passes | Biber invocations | Biber command |
| --- | ---: | ---: | ---: | ---: |
| First build | 49.50 s | 3 | 1 | 43.24 s |
| Prose-only edit | 0.74 s | 1 | 0 | — |
| Add a previously uncited entry | 15.53 s | 3 | 1 | 13.13 s |
| Edit a cited bibliography title | 15.71 s | 3 | 1 | 13.37 s |

The title-edit phase changes only the BIB file; its TeX source is byte-identical
to the preceding citation phase. Biber invocation is decided from content
hashes, not from the benchmark's phase labels.

The first TeX pass took 2.77 seconds; subsequent passes took approximately
0.73–0.78 seconds. First Biber initialization took another 0.78 seconds,
staging 0.56 seconds, and export 0.04 seconds. The 43.24-second Biber command
includes its first-use package extraction and on-demand disk I/O, not just
CPU work. Later Biber commands remain roughly 13 seconds with no additional
local downloads.

The local server delivered 92.22 MB cumulatively through the first PDF.
Every edit fetched zero additional local bytes. These are uncompressed HTTP
response-body counts, excluding headers and the externally hosted CheerpX
runtime. They are not comparable to earlier gzip-compressed v86 measurements.
Peak browser memory was not measured.

[summary.json](summary.json) preserves timings, input digests, actual engine
asset checksums, served resource hashes, and per-phase correctness checks.

## What the review changed

Three inexpensive agents supplied the fixture/verifier, initial hybrid, and
Biber bridge. Parent review and browser runs caught and corrected:

- A line-wrap-sensitive PDF assertion that falsely rejected a valid Biber edit.
- Caller-selected Biber runs, discarded auxiliary files, and writes to the
  wrong virtual filesystem.
- Same-origin worker loading, the disk's required HTTP validators, and guest
  export directory permissions.
- Overbroad rerun detection, insufficient convergence checks, and old XDV
  files masking failed engine runs.
- A package/format mismatch when using the original WasmTex package endpoint.
  Supplying packages from the frozen TinyTeX tree eliminated the
  biblatex/etoolbox errors.

The successful run uses a matching WASM-generated format and TinyTeX package
versions observed to match the native reference. It does not constitute a new
independently rebuilt engine release. Supplementary WASM ICU data/font indexing
still comes from WasmTex; CheerpX remains vendor-hosted.

## Validation and decision

The verifier passed all four per-phase comparisons and rejected deliberately
stale citation outputs in its self-test. The browser run also compiled an
invalid TeX edit after the valid phases and correctly returned failure without
an old PDF. Final valid logs contained no unresolved-reference or missing-glyph
diagnostics.

This supports the hybrid for responsive prose editing on this example.
Bibliography updates and first use are still substantial waits. It establishes
neither broad Overleaf compatibility nor production readiness: the corpus,
browser/memory coverage, resource resolver, release construction, caching across
reload, and editor integration remain narrower than the intended product.

CheerpX self-hosting requires a separate commercial licence according to its
[licensing documentation](https://cheerpx.io/docs/licensing). The prototype
does not resolve that deployment decision.

Reproduction and precise scope are in [README.md](README.md). Production
rendering code was not modified.
