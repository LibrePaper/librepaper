# Public-project LaTeX baseline

Measured: 2026-09-07T01:27:15.540Z.

Fresh browser profile per case/distribution; cold includes app boot and choose; warm is a visible body edit; reload returns to original source with persistent caches. Local uncompressed mirror, no network/CPU throttling. Memory not measured. Text agreement is a word-multiset F1, not visual or bibliography-order equivalence.

Browser: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/151.0.0.0 Safari/537.36.

Mirror manifest SHA-256: `709b30becccd4a4c6197007914dcf0b2ed8e3ef7ea0257d0252785aec9b7b577`.

These measurements assess LibrePaper's current adapters and local mirror together. A failure does not establish an engine limitation. The native reference uses the installed TeX Live, whose version may differ from the browser distribution. A PDF and matching text still require visual review; text agreement does not validate citation order or layout.

## Sources

- thesis: [fd2624a31ef0](https://github.com/latextemplates/scientific-thesis-template/tree/fd2624a31ef09946e8c0d326a5d3aa8fbea95c4e). Unlicense; retain the archive's notices for included components.
- acmart: [ad98567f427b](https://github.com/borisveytsman/acmart/tree/ad98567f427b9cd854a8d0c0d0870c55333f5c99). LPPL-1.3c; see LICENSE in the archive.
- biblatex: [7f0435bfcff2](https://github.com/plk/biblatex/tree/7f0435bfcff2da18b820ab1adf39a2950fa2e692). LPPL-1.3-or-later; see README.md in the archive.

## Native references

| Case | Engine | Pages | Status |
| --- | --- | ---: | --- |
| acm-conference | pdflatex | 6 | reference-ready |
| acm-journal | pdflatex | 11 | reference-ready |
| thesis | lualatex | 59 | reference-ready |
| biber-related | xelatex | 2 | reference-ready |
| biber-sorting | xelatex | 1 | reference-ready |
| multifile | pdflatex | 4 | reference-ready |
| packages | pdflatex | 2 | reference-ready |
| unicode-fonts | xelatex | 1 | reference-ready |

- pdfTeX 3.141592653-2.6-1.40.27 (TeX Live 2025/nixos.org)
- This is LuaHBTeX, Version 1.22.0 (TeX Live 2025/nixos.org)
- XeTeX 3.141592653-2.6-0.999997 (TeX Live 2025/nixos.org)

## Browser results

Cold includes initialization and first compilation; edit inserts visible text before the document ends. Reload recompiles the original input after page navigation, with the same browser profile. MB means uncompressed response-body bytes served by the local mirror, not compressed network transfer. A fast failure is not a fast preview.

| Distribution | Case | Cold status | Pages | Cold MB | Cold s | Edit s | Reload MB | SyncTeX | Text F1 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| swiftlatex-pdftex | acm-conference | failed | 0 | 14.50 | 0.75 | 0.24 | 10.42 | no | — |
| swiftlatex-pdftex | acm-journal | failed | 0 | 14.50 | 0.80 | 0.23 | 10.42 | no | — |
| swiftlatex-pdftex | thesis | failed | 0 | 14.49 | 0.70 | 0.17 | 10.42 | no | — |
| swiftlatex-pdftex | biber-related | failed | 0 | 14.64 | 0.59 | 0.16 | 10.56 | no | — |
| swiftlatex-pdftex | biber-sorting | failed | 0 | 14.64 | 0.63 | 0.16 | 10.56 | no | — |
| swiftlatex-pdftex | multifile | pdf-produced | 4 | 21.23 | 1.54 | 0.32 | 17.16 | no | 1.00 |
| swiftlatex-pdftex | packages | failed | 0 | 14.88 | 0.75 | 0.15 | 10.81 | no | — |
| swiftlatex-pdftex | unicode-fonts | failed | 0 | 14.64 | 0.61 | 0.15 | 10.56 | no | — |
| busytex | acm-conference | failed | 0 | 220.11 | 18.57 | 17.35 | 0.06 | no | — |
| busytex | acm-journal | failed | 0 | 220.11 | 18.57 | 16.01 | 0.06 | no | — |
| busytex | thesis | failed | 0 | 220.11 | 6.36 | 3.81 | 0.06 | no | — |
| busytex | biber-related | failed | 0 | 220.11 | 4.65 | 1.67 | 0.06 | no | — |
| busytex | biber-sorting | failed | 0 | 220.11 | 4.32 | 1.67 | 0.06 | no | — |
| busytex | multifile | failed | 0 | 220.11 | 9.60 | 1.37 | 0.06 | no | — |
| busytex | packages | failed | 0 | 220.11 | 5.19 | 1.06 | 0.06 | no | — |
| busytex | unicode-fonts | failed | 0 | 220.11 | 3.52 | 1.52 | 0.06 | no | — |
| texlyre-busytex | acm-conference | failed | 0 | 141.24 | 1.94 | 0.24 | 0.06 | no | — |
| texlyre-busytex | acm-journal | failed | 0 | 141.24 | 1.43 | 0.25 | 0.06 | no | — |
| texlyre-busytex | thesis | failed | 0 | 684.00 | 8.20 | 3.05 | 0.09 | no | — |
| texlyre-busytex | biber-related | failed | 0 | 684.10 | 10.78 | 2.28 | 0.15 | no | — |
| texlyre-busytex | biber-sorting | failed | 0 | 694.72 | 7.61 | 1.67 | 0.15 | no | — |
| texlyre-busytex | multifile | pdf-produced | 4 | 684.00 | 5.86 | 0.92 | 0.06 | yes | 1.00 |
| texlyre-busytex | packages | failed | 0 | 684.39 | 7.76 | 0.31 | 0.45 | no | — |
| texlyre-busytex | unicode-fonts | failed | 0 | 684.00 | 17.51 | 6.21 | 0.06 | no | — |

## Failures and review notes

- swiftlatex-pdftex/acm-conference: I can't find file `xkeyval'.
- swiftlatex-pdftex/acm-journal: I can't find file `xkeyval'.
- swiftlatex-pdftex/thesis: LaTeX Error: File `kvoptions-patch.sty' not found.
- swiftlatex-pdftex/thesis: requested lualatex, selected pdflatex.
- swiftlatex-pdftex/biber-related: Fatal Package fontspec Error: The fontspec package requires either XeTeX or
- swiftlatex-pdftex/biber-related: requested xelatex, selected pdflatex.
- swiftlatex-pdftex/biber-sorting: Fatal Package fontspec Error: The fontspec package requires either XeTeX or
- swiftlatex-pdftex/biber-sorting: requested xelatex, selected pdflatex.
- swiftlatex-pdftex/packages: Package siunitx Error: LaTeX kernel too old.
- swiftlatex-pdftex/unicode-fonts: Fatal Package fontspec Error: The fontspec package requires either XeTeX or
- swiftlatex-pdftex/unicode-fonts: requested xelatex, selected pdflatex.
- busytex/acm-conference: pdfTeX error (font expansion): auto expansion is only possible with scalable fonts.
- busytex/acm-journal: pdfTeX error (font expansion): auto expansion is only possible with scalable fonts.
- busytex/thesis: Package babel Error: Unknown option 'ngerman'. Either you misspelled it
- busytex/thesis: requested lualatex, selected xelatex.
- busytex/biber-related: LaTeX Error: File `biblatex.sty' not found.
- busytex/biber-sorting: LaTeX Error: File `biblatex.sty' not found.
- busytex/multifile: pdfTeX error: /bin/busytex (file ecti1095): Font ecti1095 at 600 not found
- busytex/packages: LaTeX Error: File `tikz.sty' not found.
- busytex/unicode-fonts: Package fontspec Error: The font "Libertinus Serif" cannot be found.
- texlyre-busytex/acm-conference: I can't find file `xkeyval'.
- texlyre-busytex/acm-journal: I can't find file `xkeyval'.
- texlyre-busytex/thesis: Package babel Error: Unknown option 'ngerman'.
- texlyre-busytex/thesis: requested lualatex, selected xelatex.
- texlyre-busytex/biber-related: LaTeX Error: File `biblatex.sty' not found.
- texlyre-busytex/biber-sorting: Error: No biber module factory found. Ensure biber.js is loaded.
- texlyre-busytex/packages: LaTeX Error: File `biblatex.sty' not found.
- texlyre-busytex/unicode-fonts: Package fontspec Error:

## Remaining evaluation

- Integrate and pin the Siglum and WasmTex candidates before comparing them with this baseline; neither has been benchmarked here.
- Measure repeated trials, realistic network conditions, worker/WASM memory, and Safari on actual hardware. This local run supplies none of those numbers.
- Inspect PDF layout, bibliography content and ordering, and source/PDF navigation. SyncTeX here means a valid gzip with a SyncTeX header, not verified navigation accuracy.
