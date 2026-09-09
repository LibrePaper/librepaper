# Browser TeX/BibTeX engine comparison

Compared 2026-09-07. **Recommendation: use WasmTex's 2026 engine layer as the
first LibrePaper-controlled distribution.** TeXlyre BusyTeX is the runner-up;
defer Typeward. This selects where to invest integration work, not a
production-ready replacement. No production adapter or mirror was changed.

The revised requirement is browser TeX and real BibTeX, with local LibrePaper
finding compatible Biber when required. Browser Biber does not affect this
ranking. TeXlyre's Biber defects and WasmTex's full-Biber backend limitation
therefore do not disqualify either.

## Versions and methods

| Candidate | Inspected source | Examined artifacts |
| --- | --- | --- |
| WasmTex | 44c5861fcdf729838205b00b96ac9509bc7fb677 | 2026-8b7946970153c52e; previous hybrid used 2025-2b3c48691f3cd6d7 |
| TeXlyre build | f544a51a99e7d3978bb70608e927a9a23f96d4a7 | Existing mirror of assets-v1.4.0 |
| TeXlyre API | f3c8780e85939ced63133501d66b6386d89f69e4 | Latest listed assets release remains assets-v1.4.0 |
| Typeward | 7964df503048cd27af9f83e7d25c07ac10a5bcd8, package 0.2.5-alpha | v0.2.4-alpha; no v0.2.5-alpha GitHub release found |

This combines a new Chromium WasmTex check, Typeward artifacts run directly
under Node, source inspection, and earlier TeXlyre measurements. **It is not
a controlled performance leaderboard.** No independent engine build was
completed. Safari/mobile peak memory remains unmeasured.

## WasmTex: preferred foundation

The [new browser check](wasmtex-2026-check.mjs) uses direct pdfTeX and BibTeX
engines, with preamble snapshots disabled. It loads the published 2026 engine
release and package snapshot 2026-ba38749b8714505a. Engine files are verified
against the manifest. Package response hashes record what was used; they do
not independently establish upstream snapshot immutability.

The document uses siunitx, an external bibliography database, plain.bst and
a citation. TeX -> BibTeX -> TeX -> TeX succeeded, followed by a prose edit
retaining the workers and auxiliary files.

- Actual runtime: pdfTeX 1.40.29 / TeX Live 2026, LaTeX 2026-06-01,
  L3 2026-08-10, BibTeX 0.99e.
- Extracted PDF text includes "Engine comparison: 12 m. Reference [1]."
  and the expected Knuth/The TeXbook bibliography.
- Final logs contain no unresolved references.
- First complete build: 16.79 seconds including upstream resource fetching.
  Subsequent prose edit: 0.193 seconds without another BibTeX run.
- Local proxy served 12,619,800 bytes across 39 engine/resource requests.
  These count decoded response bodies forwarded without HTTP compression,
  not compressed internet transfer or browser memory.
- Distinct successful assets comprise 5,807,474 engine/glue/format bytes and
  6,785,176 resource bytes. A bare "plain" lookup returned 404 and the
  extension retry successfully found plain.bst.

[Full result and resource hashes](wasmtex-2026-result.json);
[PDF](wasmtex-2026.pdf). These are single observations on a small document.
The [2025 standalone BibTeX check](wasmtex-2025-bibtex-result.json) passed too.

This establishes a working direct browser pdfTeX/BibTeX sequence. It does not
resolve every detail of the earlier whole-wrapper multifile failure. Nested
auxiliary files and custom styles still need to pass the real LibrePaper adapter.
The [earlier XeTeX hybrid](../hybrid/REPORT.md) additionally matched native
PDF text/page counts and real-Biber outputs across four scenarios, but those
results used 2025 engines and must not be attributed to this 2026 build.

The 2026 manifest publishes separate WASM modules: pdfTeX 1.67 MB, BibTeX
0.206 MB, XeTeX 3.49 MB and LuaHBTeX 6.61 MB, excluding formats/resources.
pdfTeX's format adds 3.66 MB before transport compression. Module separation
is already built and published; LibrePaper would not first have to split a
combined engine.

Build recipes pin TeX Live sources and the Emscripten image. Engine families
have build receipts and corresponding-source archives. This is an auditable
starting point, not proof that we can reproduce the builds yet.

Maintenance includes custom kpathsea hooks, worker state restoration,
PDF-library adaptations and engine-specific build recipes. Earlier
package/format mismatch and stale-XDV behavior demonstrate why LibrePaper should
own a narrow controller instead of assuming the whole SDK resolves integration.
Defer experimental checkpoint features.

Small downloads do not prove low memory. The earlier multifile pdfTeX wrapper
experiment reported about 340.5 MiB of WASM linear memory plus a 64 MiB initial
snapshot. That is not this fixture's memory measurement or peak browser memory.

## TeXlyre BusyTeX: credible runner-up

This is the current TeX Live 2026 fork, not upstream BusyTeX 2023. It includes
pdfTeX, XeTeX, LuaHBTeX, BibTeX8 and makeindex. Its actual CI pins Emscripten
5.0.4, although README manual instructions use "tot". Recent engine CI builds
succeeded; its font/index and format-generation work is useful.

[Earlier corpus measurements](../../../corpus/MEASUREMENTS.md) demonstrated
article/paper, XeTeX/font and combined package cases with bundled resources.
The [ACM diagnosis](../existing/REPORT.md) isolated a fixable package lookup
chain. Neither finding means that the underlying engine is broken.

Its disadvantage for LibrePaper is shipped delivery:

- Existing combined WASM: 32.51 MB.
- Basic data bundle: 92.79 MB, already internally LZ4-compressed.
- [The bundle audit](../PACKAGING.md) found overlapping package trees and
  formats/resources for multiple engines.
- Current pipeline source still enables all available data bundles on an
  unresolved package. It also offers a BibTeX8 fallback when Biber is missing;
  LibrePaper must prevent that substitution.

These sizes are not inherent TeX requirements. Smaller resources and split
engines are possible work. Split targets exist in the Makefile, but current
API history removed split-engine instructions, and we have not verified a
supported published split release. A hypothetical optimized build should not
be compared with today's verified WasmTex assets as if both were ready.

Choose this foundation instead if the larger corpus, especially LuaTeX/fonts,
shows materially better compatibility than WasmTex. Today it requires more
startup packaging work to reach the desired automatic, lightweight setup.

## Typeward: promising structure, premature adoption

Positive source findings: separate modules, pinned build tooling, per-engine
core bundles, asset version checks, cached compiled modules, fresh instances
per invocation, and lazy filesystem support. Smoke scripts exercise more than
the README's conservative "instantiates" wording suggests.

The inspected release workflow failed only at npm publication. Engine builds,
smoke and asset packaging succeeded. Do not call this an engine-build failure.
Source package metadata and published artifact versions differ, so consumers
must use a matching release or build their own.

Four downloaded v0.2.4-alpha assets passed published size/SHA-256 checks:
pdfLaTeX, BibTeXu, ICU and the pdfLaTeX core. Running the published pdfLaTeX
under Node produced a one-page 11,287-byte PDF with these banners:

    pdfTeX 1.40.29 (TeX Live 2026)
    LaTeX2e <2023-11-01> patch level 1
    L3 programming layer <2024-01-22>

The resource assembler uses Ubuntu 24.04 packages; formats are subsequently
redumped with WASM. The issue is older macro resources, not incompatible
native format dumps.

BibTeXu's smoke failed with the small core, which omits plain.bst. Supplying
that style in the project directory produced a successful 168-byte BBL
containing Knuth. This diagnoses resource/setup work, not a broken binary or
a proven browser-wrapper failure. The Typeward browser wrapper was not tested.

Published compressed sizes: pdfLaTeX core 28.36 MB; pdfLaTeX archive 0.533 MB;
BibTeXu 0.357 MB; ICU 9.68 MB. BibTeXu uses ICU too. These archive sizes are not
a measured browser first-PDF budget. Bibliography styles need additional delivery.
SyncTeX forward/reverse methods currently return empty arrays; a separate
parser could replace them.

[Recorded artifact evidence](typeward-evidence.json). There is not yet a
demonstrated advantage sufficient to offset the older resources and additional
integration validation.

## Ownership and adoption

Use one pinned, LibrePaper-hosted release based on WasmTex's engine layer. Load
the selected project engine and required helpers automatically. Ordinary
BibTeX needs no local application. Biber requests use connected local LibrePaper
with a compatible Biber, or explain that requirement.

Own immutable static resources, matching formats/fonts, cache namespaces by
release digest, and compile orchestration. Preserve useful auxiliary files,
rerun bibliography when its actual inputs change, and reject stale outputs.
The localhost proxy is test infrastructure; production resource delivery can
be static and requires no compilation server.

First mirror verified binaries. Independently reproduce a pdfTeX/BibTeX build
before making this the default. Maintain downstream patches where tests show
they are needed; do not start a new compiler port.

Acceptance before production:

1. Multifile, ACM and package/BibTeX cases; bibliography edits; nested auxiliary
   files and project-local styles through the intended adapter.
2. Coherent resource generation and independently reproduced engine/formats.
3. Firefox/Safari, representative peak memory, cancellation, cache eviction/
   reload and correct SyncTeX navigation.

Retain TeXlyre as the alternative if these expose substantial engine defects.
Component licences differ: TeXlyre modifications are AGPL-3.0; WasmTex and
Typeward have MIT wrapper/build code with separately licensed engines and
dependencies. None is an unconditionally MIT-licensed complete distribution.
This comparison does not establish legal compatibility for LibrePaper.

## Reproduction and sources

The existing WasmTex source checkout must be at the revision above. With Node,
Chromium and network access:

    node latex/benchmark/candidates/comparison/wasmtex-2026-check.mjs
    pdftotext latex/benchmark/candidates/comparison/wasmtex-2026.pdf -

The script uses ports 8709/9711 and a fresh temporary browser profile. It
rejects a changed engine release, verifies engine files, and records package
hashes. It outputs stage results for inspection; it is not the complete
acceptance suite.

- [WasmTex engine build documentation](https://github.com/corca-ai/wasmtex/blob/44c5861fcdf729838205b00b96ac9509bc7fb677/docs/engine.md)
- [WasmTex 2026 manifest](https://corca-ai.github.io/wasmtex/wasmtex/2026/manifest.json)
- [WasmTex licensing/provenance](https://github.com/corca-ai/wasmtex/blob/44c5861fcdf729838205b00b96ac9509bc7fb677/docs/licensing.md)
- [TeXlyre build source](https://github.com/TeXlyre/texlyre-busytex-build/tree/f544a51a99e7d3978bb70608e927a9a23f96d4a7)
- [TeXlyre pipeline](https://github.com/TeXlyre/texlyre-busytex-build/blob/f544a51a99e7d3978bb70608e927a9a23f96d4a7/web/busytex_pipeline.js)
- [Typeward source](https://github.com/typeward/texlive-wasm/tree/7964df503048cd27af9f83e7d25c07ac10a5bcd8)
- [Typeward resource assembly](https://github.com/typeward/texlive-wasm/blob/7964df503048cd27af9f83e7d25c07ac10a5bcd8/scripts/fetch-tds.sh)
- [Typeward release](https://github.com/typeward/texlive-wasm/releases/tag/v0.2.4-alpha)
- [Typeward release workflow](https://github.com/typeward/texlive-wasm/actions/runs/29328586330)
