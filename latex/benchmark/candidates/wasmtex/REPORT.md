# WasmTex evaluation

Evaluated [checkout `44c5861fcdf729838205b00b96ac9509bc7fb677`](https://github.com/corca-ai/wasmtex/tree/44c5861fcdf729838205b00b96ac9509bc7fb677)
(2026-09-05, package version `0.1.0`). The published 2025 engine set is release
`2025-2b3c48691f3cd6d7`, with build receipts in `downloads/manifest-2025.json`.
Its receipt source revisions are `0dddc924cc6e69bd2a4b4630e02efe414f84515e`
(pdfTeX, XeTeX, LuaHBTeX) and `d017693840ba301a7723305646e5a53862677fa8`
(BibTeX8/MakeIndex). The TeX Live package endpoint is the immutable-looking
snapshot `2025-92e10d3241a312f0`.

The candidate is technically promising for ordinary client-side pdfLaTeX, but it
does not satisfy LibrePaper's browser-only bibliography requirement. WasmTex's own
[`docs/bibliography.md`](https://github.com/corca-ai/wasmtex/blob/44c5861fcdf729838205b00b96ac9509bc7fb677/docs/bibliography.md) says the default biblatex implementation is
“biblatex-lite”, covering a numeric/author-year subset, while full-fidelity Biber
requires a registered server backend. [`docs/howto.md`](https://github.com/corca-ai/wasmtex/blob/44c5861fcdf729838205b00b96ac9509bc7fb677/docs/howto.md) also states that the
browser `WasmTex` UI is pdfLaTeX-only; XeLaTeX/LuaLaTeX require the headless
compiler and matching hosted assets. A server Biber dependency is disallowed for
this use case, so this is a candidate for a constrained preview, not a complete
LibrePaper renderer.

## Published asset and source evidence

The manifest enumerates 26,360,227 bytes of 2025 engine assets and notices. The
headline engine sizes are:

| Asset | Bytes |
| --- | ---: |
| pdfTeX WASM | 1,652,437 |
| pdfTeX format | 3,633,014 |
| XeTeX WASM | 3,411,963 |
| XeTeX format.gz | 3,903,130 |
| LuaHBTeX WASM | 5,650,969 |
| LuaTeX format.gz | 3,129,517 |

The source has authored worker/glue code and [TeX Live build scripts](https://github.com/corca-ai/wasmtex/tree/44c5861fcdf729838205b00b96ac9509bc7fb677/wasm-build) rather than
only an opaque bundle. It records Emscripten 3.1.46, TeX Live source references,
per-file SHA-256 receipts, and corresponding-source archives in its licensing
documentation ([engine/build guide](https://github.com/corca-ai/wasmtex/blob/44c5861fcdf729838205b00b96ac9509bc7fb677/docs/engine.md),
[license/provenance guide](https://github.com/corca-ai/wasmtex/blob/44c5861fcdf729838205b00b96ac9509bc7fb677/docs/licensing.md),
and [corresponding-source procedure](https://github.com/corca-ai/wasmtex/blob/44c5861fcdf729838205b00b96ac9509bc7fb677/docs/corresponding-source.md)).
The repository's checked-out public directory does not contain
the WASM assets; consumers must self-host or sync the hosted release. The worker
URL must be same-origin with the application: a direct local page using the
GitHub Pages URL failed with `SecurityError: Failed to construct 'Worker'` even
though the worker response advertises CORS. The harness had to proxy the exact
published paths locally. A LibrePaper integration therefore owns an asset proxy or
mirrored release, CDN availability, mirror snapshot selection, and receipt
verification.

## Browser smoke evidence

The reproducible harness is `bench.mjs`, `harness/index.html`, and
`harness/vite-bench.config.mjs`. It runs Chromium on port 9702, Vite/proxy on
8702, uses `WasmTexCompiler` from the pinned checkout, and supplies the public
2025 assets plus TeX Live snapshot. It was run as:

```sh
cd latex/benchmark/candidates/wasmtex
git clone https://github.com/corca-ai/wasmtex.git source
git -C source checkout 44c5861fcdf729838205b00b96ac9509bc7fb677
npm ci --ignore-scripts                         # in source/
cd ../../../../
node latex/benchmark/candidates/wasmtex/bench.mjs multifile
```

The manifest and corresponding-source identities can be reconstructed without
trusting a moving branch:

```sh
curl -fL https://corca-ai.github.io/wasmtex/wasmtex/2025/manifest.json \
  -o downloads/manifest-2025.json
curl -fL https://corca-ai.github.io/wasmtex/wasmtex/2025/wasmtex-pdftex.wasm \
  -o downloads/wasmtex-pdftex.wasm
sha256sum downloads/wasmtex-pdftex.wasm  # compare with manifest entry
```

The run uses a fresh compiler for each phase. Therefore “edit” and “reload” are
warm asset-cache/reinitialization observations, not the existing benchmark's
warm-worker edit protocol. The reported `pdfBytes` is PDF output size, not network
download bytes; timings are exploratory under concurrent agent activity and are
not an engine ranking. `results/` contains the logs and PDFs.

| Case / phase | Result | Pages | Time | PDF bytes | SyncTeX |
| --- | --- | ---: | ---: | ---: | ---: |
| multifile / cold | PDF produced, needs review | 3 | 10.7 s | 207,425 | 5,856 B gzip |
| multifile / edit | PDF produced, needs review | 4 | 0.7 s | — | — |
| multifile / reload | PDF produced, needs review | 3 | 0.7 s | — | 5,856 B gzip |

The cold result proves browser pdfTeX can load the multi-file tree, local style,
PNG/PDF figures, and emit valid gzip SyncTeX. Its phase telemetry measured a
64 MiB initial heap snapshot and 356,974,592 bytes of current WASM linear memory
(about 340.5 MiB) for this small document. The PDF was three pages versus the
four-page native reference because WasmTex emitted no `main.bbl`; diagnostics
reported undefined `knuth1984` and `lamport1994` citations. This is a real
classic BibTeX pipeline failure in the tested run, independent of the separate
full-Biber limitation. The edit inserted visible text and produced four pages,
but bibliography remained unresolved.

An attempted `packages`, `biber-related`, and `unicode-fonts` run exceeded the
driver's 60-second evaluation timeout before recording a phase result. It is
therefore untested rather than classified as an engine incompatibility. The
repository's own documentation and source inspection are sufficient to classify
full Biber as unsupported without a server backend.

## Compatibility assessment

- Ordinary pdfLaTeX: executable in Chromium with hosted TeX Live assets; the
  multifile smoke passed PDF generation, figures, reload, and SyncTeX production.
- Classic BibTeX: failed to inject `main.bbl` in the multifile smoke, leaving
  citations undefined and changing page count. This needs a focused upstream
  issue/retest before adoption.
- biblatex/Biber: browser default is the documented lite JS subset; full Biber
  is an explicit server backend. This violates the browser-only requirement.
- XeLaTeX/LuaLaTeX: headless-only path according to the candidate docs, with
  separate WASM/format assets and automatic detection. The focused run timed out
  before producing evidence, so no pass is claimed.
- SyncTeX: raw gzipped output had a valid SyncTeX payload in the multifile cold
  and reload results. Navigation accuracy was not tested.
- Reload/cache: a new compiler reused the browser profile's persistent cache and
  fell from 10.7 s cold to 0.7 s reload in this local run; this is only a
  cache/reinitialization measurement.

## Recommended next fix

Do not integrate WasmTex as LibrePaper's complete browser LaTeX backend yet. If it is
kept as an experiment, first fix or isolate the classic BibTeX rerun/bbl injection
path, add an actual browser test that checks citation text/order, and decide where
LibrePaper will self-host the exact engine release and immutable TeX Live snapshot.
Full Biber remains a hard blocker unless the requirement changes to allow an
explicit server backend. Repeat Xe/Lua and packages with a longer driver timeout,
then test SyncTeX coordinate mapping and memory under the real editor worker.
