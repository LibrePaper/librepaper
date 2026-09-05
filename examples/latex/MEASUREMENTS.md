# The distributions, measured

Written by `node web/scripts/latex-check.mjs --measure` against the local
mirror `node latex/mirror.mjs` builds, in headless Chromium, on one machine.
Bytes are what the mirror actually served, counted by `latex/serve.mjs`, with
Cache Storage emptied before each distribution and the byte tally reset before
each compile -- so a document's row is what a cold cache costs for that
document alone, over and above the up-front row.

| distribution | what | bytes fetched | cold | warm | pages |
| --- | --- | --- | --- | --- | --- |
| swiftlatex-pdftex | up front | 1.94 MB | 0.1 s | -- | -- |
| swiftlatex-pdftex | article | 17.38 MB | 1.2 s | 0.2 s | 3 |
| swiftlatex-pdftex | paper | 0.26 MB | 0.2 s | 0.2 s | 4 |
| swiftlatex-pdftex | broken | 0.00 MB | 0.1 s | 0.1 s | 0 |
| swiftlatex-pdftex | xetex | 0.09 MB | 0.0 s | 0.0 s | 0 |
| swiftlatex-xetex | up front | 3.97 MB | 0.0 s | -- | -- |
| swiftlatex-xetex | article | 29.54 MB | 0.6 s | 0.2 s | 3 |
| swiftlatex-xetex | paper | 0.10 MB | 0.2 s | 0.2 s | 4 |
| swiftlatex-xetex | broken | 0.00 MB | 0.0 s | 0.0 s | 0 |
| swiftlatex-xetex | xetex | 1.01 MB | 0.4 s | 0.3 s | 0 |
| busytex | up front | 34.84 MB | 0.1 s | -- | -- |
| busytex | article | 182.73 MB | 2.3 s | 0.6 s | 0 |
| busytex | paper | 0.00 MB | 0.9 s | 0.9 s | 0 |
| busytex | broken | 0.00 MB | 0.2 s | 0.2 s | 0 |
| busytex | xetex | 0.00 MB | 1.0 s | 0.9 s | 1 |

The page counts a TeX Live on a desk gives the same documents are in
`examples/latex/pages.json`, written by `node latex/texlive.mjs`.

Two things the table will mislead about if read quickly.

The **up front** row is what `choose` fetched before it resolved, and
that is not the same as what a distribution costs before its first page.
Both distributions defer most of their weight: SwiftLaTeX fetches a
module and then the LaTeX format and every package from inside the first
compile, and BusyTeX's pipeline constructs itself and then pulls its TeX
Live down when a document arrives. So the honest number for the card is
the up-front row **plus** the first document's row, and the first
document's row for a second document is the one under `paper`.

The **article** row is therefore the first-compile cost and the `paper`
row the steady-state one. That `paper` costs a quarter of a megabyte on
SwiftLaTeX and nothing at all on BusyTeX is the whole of the trade
between them, in two numbers.
