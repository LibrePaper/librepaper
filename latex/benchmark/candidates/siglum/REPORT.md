# Siglum candidate evaluation

Measured 2026-09-07 in headless Chromium 151, using Siglum source commit
[`00db6721bc7c50dd651761da1c07a1ef3285dbcf`](https://github.com/SiglumProject/siglum/tree/00db6721bc7c50dd651761da1c07a1ef3285dbcf)
(`v0.1.4`). The reproducible harness is [runner.mjs](runner.mjs), with the
required local server on 8701 and Chromium DevTools on 9701. It uses the
eight-case LibrePaper corpus, a fresh profile for each case, then edit and reload
in that profile. No CTAN proxy was enabled, so missing packages are reported as
failures. The run was concurrent with other agent work; timings are
exploratory and are not used to rank engines.

The harness waits for page initialization, then resets the byte counter before
the cold compile. Thus the cold byte/time columns below measure compilation and
lazy bundle loading after initialization; they exclude the page's initial WASM,
loader, and manifest requests. Reload is measured after navigation and a fresh
readiness wait.

## Pinned runtime evidence

The published [Siglum CDN](https://cdn.siglum.org/tl2025/) assets used by the
run were downloaded on the measurement date and SHA-256 hashed locally:

| Asset | Response bytes | SHA-256 |
| --- | ---: | --- |
| `busytex.wasm` | 30,767,701 | `3efcd116f16e058048f0870d5fbe75eafe6441063d9e7f83e617bf57102645bd` |
| `busytex.js` | 298,998 | `53374fbb04e9b485c5854ae107cb68c3d3b79735e684c6049a7597013a23ff2b` |
| `siglum-bundles-v0.1.0.tar.gz` | 199,133,079 | `90dcc717a4361c466e035492b10f9c7573b5b4062f6fad88892e41a63e443484` |

The archive extracts to about 195 MB of gzip bundle files. Siglum's README
advertises a roughly 29 MB WASM engine and roughly 16 MB pdfLaTeX core; those
are claims, while the table above is measured. A successful multifile cold
compile phase served 60.3 MB of response bodies, mostly the on-demand
`cm-super` bundle (about 59 MB compressed) plus the compile's other lazy
bundles. The initial WASM request is excluded from this phase counter and is
30.8 MB by itself. The bundle archive's `.data.gz` files are compressed
containers; these byte counts are HTTP response bytes, not the virtual
filesystem size.

## Corpus result

| Case | Cold | Edit | Reload | Cold compile-phase bytes | Main finding |
| --- | --- | --- | ---: | ---: | --- |
| acm-conference | failed | failed | failed | 0.3 MB | `xkeyval.sty` absent |
| acm-journal | failed | failed | failed | 0.3 MB | `xkeyval.sty` absent |
| thesis | failed | failed | failed | 97.6 MB | requested LuaLaTeX falls into XeLaTeX branch; xelatex format absent |
| biber-related | failed | failed | failed | 77.9 MB | `csquotes.sty` absent; no Biber runner |
| biber-sorting | failed | failed | failed | 77.9 MB | `csquotes.sty` absent; no Biber runner |
| multifile | PDF, 3 pages | PDF, 4 pages | PDF, 3 pages | 60.3 MB | SyncTeX present; native reference is 4 pages; text F1 0.931; undefined citations |
| packages | failed | failed | failed | 36.4 MB | `booktabs.sty` absent |
| unicode-fonts | failed | failed | failed | 76.3 MB | `amsmath.sty` absent |

The multifile result is the one successful PDF-producing case. Its edit output
contains the inserted `Benchmark edit.` text and remains readable, and all
three phases expose a SyncTeX payload. The cold/reload PDF has three pages
versus the native four-page reference, with text multiset F1 0.9306. Logs also
show undefined natbib citations because Siglum only runs pdfTeX/XeTeX and does
not run BibTeX or Biber. A successful PDF therefore does not establish
compatibility.

The complete machine-readable output is [results/report.json](results/report.json)
and per-case logs/PDFs are under the ignored `results/` directory.

## Engine, bibliography, cache, and source inspection

The pinned [`src/worker.js`](https://github.com/SiglumProject/siglum/blob/00db6721bc7c50dd651761da1c07a1ef3285dbcf/src/worker.js)
has a pdfLaTeX path and an else path that runs XeLaTeX then
`xdvipdfmx`; there is no BibTeX or Biber invocation. Although the public API
documents `pdflatex`, `xelatex`, and `lualatex`, requesting `lualatex` selects
the XeLaTeX path in this revision. The worker uses `--synctex=-1` and returns
the uncompressed SyncTeX text, which the harness verified by nonzero length.

The pinned [`@siglum/filesystem` 0.2.1](https://www.npmjs.com/package/@siglum/filesystem/v/0.2.1)
implementation uses IndexedDB/OPFS for manifests,
bundles, format files, auxiliary files, and PDFs. In this run, reload of the
successful case served about 1.8 MB and reused the runtime/bundle cache. The
compiler's document cache was disabled by the harness to force recompilation;
Siglum supports it when enabled. Bundle and format caches are still exercised.

Package resolution is a finite checked-in TeX Live 2025 index plus lazy bundle
fetching. With the CTAN proxy disabled, several corpus files fail on packages
that are not in the shipped bundles. Enabling the proxy would add an external
runtime service and would make package bytes/version provenance mutable unless
LibrePaper operated and pinned that proxy itself.

## Feasibility and maintenance

The candidate is technically promising for a narrow pdfLaTeX/XeLaTeX browser
preview: the WASM runtime starts, lazy bundles work, multi-file assets work,
PDFs are produced, edits remain in the worker, reload reuses browser storage,
and SyncTeX data is emitted. The measured corpus shows the current bundle
selection is incomplete for ordinary ACM, siunitx, biblatex, and Unicode cases,
and the one successful case differs materially in pages and bibliography.

Adopting it would require LibrePaper to pin and serve the WASM, loader, all bundle
metadata/data, the filesystem package, and likely an operated CTAN proxy;
maintainers would need to keep the BusyTeX/TeX Live engine, format files,
bundle offsets, package index, and browser storage schema coherent. If Siglum
disappeared, LibrePaper could serve the already downloaded assets, but would own
the bundle-generation/update pipeline and would need to implement or integrate
BibTeX/Biber, LuaLaTeX, missing package coverage, and security review of the
proxy/update path. The repository's pinned [build guide](https://github.com/SiglumProject/siglum/blob/00db6721bc7c50dd651761da1c07a1ef3285dbcf/docs/building.md)
describes updating TeX Live and regenerating bundles, but the checked-out source does not contain the large
WASM or data assets, so a fully independent rebuild also depends on the
BusyTeX build inputs and TeX Live archives.

## Reproduction

```sh
mkdir -p src
git clone https://github.com/SiglumProject/siglum.git src
git -C src checkout 00db6721bc7c50dd651761da1c07a1ef3285dbcf
mkdir -p assets/bundles node_modules/@siglum/filesystem
curl -fL -o assets/busytex.wasm https://cdn.siglum.org/tl2025/busytex.wasm
curl -fL -o assets/busytex.js https://cdn.siglum.org/tl2025/busytex.js
curl -fL -o assets/siglum-bundles-v0.1.0.tar.gz https://cdn.siglum.org/tl2025/siglum-bundles-v0.1.0.tar.gz
tar -xzf assets/siglum-bundles-v0.1.0.tar.gz -C assets/bundles
npm pack @siglum/filesystem@0.2.1 --pack-destination assets
npm pack blake3-wasm@2.1.5 --pack-destination assets
tar -xzf assets/siglum-filesystem-0.2.1.tgz -C node_modules/@siglum/filesystem --strip-components=1
tar -xzf assets/blake3-wasm-2.1.5.tgz -C node_modules
mv node_modules/package node_modules/blake3-wasm
node runner.mjs
```

The runner and harness are intentionally confined to this candidate directory;
they do not edit the shared benchmark runner, baseline, mirror, or production
files.
