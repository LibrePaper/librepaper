# Browser LaTeX evaluation

This corpus gives LibrePaper public documents to test before choosing a browser
compiler. It complements the small regression fixtures in `latex/corpus/`.
It does not change the editor, the offered distributions, or the package mirror.

The first measured run is in [BASELINE.md](BASELINE.md), with machine-readable
results and source identities in [baseline.json](baseline.json).
The subsequent parallel candidate experiments and current recommendation are in
[candidates/ASSESSMENT.md](candidates/ASSESSMENT.md).

## Cases

| Case | Input | Native engine | Coverage |
| --- | --- | --- | --- |
| acm-conference | ACM's generated `sigconf.tex` | pdfLaTeX | Conference layout, BibTeX, tables, PDF/PNG figures |
| acm-journal | ACM's generated `acmsmall.tex` | pdfLaTeX | Journal layout and bibliography |
| thesis | Scientific Thesis Template's `main-english.tex` | LuaLaTeX | Long document, KOMA-Script, Biber, TikZ, fonts |
| biber-related | biblatex example 90 | XeLaTeX | Related works, translations, Unicode bibliography |
| biber-sorting | biblatex example 91 | XeLaTeX | Different citation and bibliography sorting contexts |
| multifile | Existing LibrePaper `paper/` | pdfLaTeX | Chapters, local style, relative asset paths |
| packages | Existing LibrePaper `packages/` | pdfLaTeX | siunitx, TikZ, booktabs, biblatex with BibTeX |
| unicode-fonts | Authored fixture in this directory | XeLaTeX | Named Libertinus text/math fonts, Greek and Cyrillic |

The public projects are pinned by commit and archive SHA-256 in
[sources.json](sources.json). Downloads and extracted upstream notices remain
under ignored `.cache/`. ACM's class and samples are generated using its
docstrip instructions; the selected input files are otherwise unchanged.
The compiler input receives an explicit engine directive, with the same input
sent to native TeX and the browser. Browser engine selection is measured, not
overridden: an adapter ignoring that directive is recorded as a mismatch.

These examples exercise useful workflows, but are not a representative survey
of authors' projects or evidence of complete Overleaf compatibility.

## Run

From the repository root:

```sh
node latex/benchmark/prepare.mjs
node latex/benchmark/check.mjs
node latex/benchmark/native.mjs
node latex/benchmark/browser.mjs
node latex/benchmark/report.mjs
```

Developer requirements: Node with `fetch` and `WebSocket`, `tar`, an installed
TeX Live with `latexmk`/Biber, Poppler's `pdfinfo`/`pdftotext`, Chromium, and the
existing `latex/mirror/manifest.json` plus its assets. These tools generate and
evaluate fixtures locally; they are never required by LibrePaper authors or readers.
The native run disables shell escape and latexmk configuration files. It is a
reference build of the pinned public inputs, not a general-purpose sandbox for
arbitrary uploaded TeX. Upstream shell scripts and package installers are not run.

`prepare.mjs` downloads only when an archive is absent, verifies cached downloads
again, and rebuilds the extracted sources/projects. No upstream packages are
added to the developer's installed TeX Live. Native outputs, font caches, logs,
and browser outputs go in ignored `results/`.

To narrow a run:

```sh
node latex/benchmark/native.mjs --case thesis
node latex/benchmark/browser.mjs --only swiftlatex-pdftex --case multifile,packages
node latex/benchmark/browser.mjs --only texlyre-busytex
node latex/benchmark/browser.mjs --browser firefox
node latex/benchmark/report.mjs firefox
```

Browser runs default to SwiftLaTeX pdfTeX, BusyTeX, and TeXlyre BusyTeX. Hidden
distributions are tested without enabling them in the UI. SwiftLaTeX XeTeX can
also be selected with `--only swiftlatex-xetex`. Siglum and WasmTex have isolated
experimental harnesses under `candidates/`; they are not adapters for this runner.

Each browser invocation replaces `results/<browser>.json`; use a full run for
the final report. A narrowed native run updates that case and preserves the
other references. Run one browser benchmark at a time: mirror byte counters
are shared within a run. Firefox is supported by the existing driver but must
be measured separately; Chromium results say nothing about Firefox or Safari.
Use `node latex/benchmark/report.mjs chromium --snapshot` to explicitly replace
the checked-in baseline with the current results.

## What is measured

- **Cold:** a fresh browser profile for every case/distribution. The time and
  served bytes include page initialization, engine choice, and the first compile.
- **Edit:** insert visible text before the document ends, keeping the worker
  and its caches alive. There is no artificial typing debounce in this measurement.
- **Reload:** navigate the page, recreate the worker, and compile the original
  input with the profile's persistent caches intact.
- **Output:** independently parsed PDF page count and extracted text, compiler
  diagnostics, missing-glyph/reference warnings, and a readable SyncTeX header.
- **Identity:** hashes of the mirror manifest and the actual project input,
  browser user agent, native engine version, and selected/requested engines.

The native references come from the installed TeX Live, not necessarily the
same release as a browser engine. Text comparison is enabled only for a clean
native result with the same input hash. Word-multiset F1 catches lost content;
it cannot establish correct ordering, positioning, fonts, or bibliography
semantics. `pdf-produced` is deliberately not called a compatibility pass.
Page differences or text agreement below 0.99 flag review in the browser runner.

The local mirror serves uncompressed files. Reported MB are response-body
bytes, not compressed wire transfer. Timing is a single local run without
network or CPU throttling; peak WASM/worker memory is not measured. Fast failures
are recorded as failures rather than as fast previews. PDFs and complete logs
remain available for inspection; browser profiles are discarded after each case.

The benchmark never enables the mirror's `--record` mode. Missing packages are
results, not a reason to silently fetch a newer package or alter the mirror.

## Next decision

Use the baseline and candidate experiments to select a bounded independent-build
experiment. Compare compatibility first, then startup cost
and edit/reload behavior. Require real Biber for the bibliography cases; a silent
BibTeX substitution or partial JavaScript bibliography backend is not equivalent.

Before selecting a production default, inspect PDF layout and citation order,
exercise actual SyncTeX navigation, repeat runs under realistic network conditions,
and measure memory and Safari on real hardware. Keep all engine/package releases
coherent rather than mixing files to make individual examples pass.
