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

# The package set, measured

Written by hand from `node latex/mirror.mjs --scheme` and a headless
SwiftLaTeX pdfTeX compile against the mirror it built, on one machine.

The mirror as `latex-check.mjs --measure` left it held only the package
files the four corpus documents asked for -- 167 of them -- so any
document that is not in the corpus failed at compile time on a missing
`.sty`. `05-SPEC-latex.md` step 3 cannot put a card in front of an author
and then refuse the packages an author uses, so the mirror now takes a
package set chosen the way a distribution chooses one: by TeX Live
collection. `node latex/mirror.mjs --scheme` mirrors
`latex-recommended`, `latex-extra`, `fonts-recommended` and `science`
(TeX Live spells the last `mathscience`) into the SwiftLaTeX package
layout, taking the file list from TeX Live's package database and the
bytes from the TeX Live on this machine.

| collection | files | bytes |
| --- | --- | --- |
| latexrecommended | 1504 | 14.1 MB |
| latexextra | 4159 | 65.0 MB |
| fontsrecommended | 3905 | 96.3 MB |
| mathscience | 636 | 14.8 MB |
| **total** | **10204** | **190.1 MB** |

A further 3095 files those collections name are ones pdfTeX has no
kpathsea format code for -- documentation, `.dtx` sources, OpenType
fonts only XeTeX can use -- and are not mirrored, because the engine
cannot ask for them. 277 more are named by the database and are not
installed on this machine; the script counts and prints them rather than
fetching them from anywhere.

With the package half of the mirror at 10371 files and 238.5 MB (the
190.1 MB above plus the 48.4 MB the corpus had already collected, which
is mostly the two ten-megabyte `.fmt` files), what each document fetches
from a cold Cache Storage on SwiftLaTeX pdfTeX:

| document | cold | warm | pages |
| --- | --- | --- | --- |
| up front (`choose`) | 4.09 MB | -- | -- |
| article | 17.54 MB | 0 B | 3 |
| paper | 0.11 MB | 0 B | 4 |
| packages | 0.35 MB | 0 B | 0 -- see below |

`article` and `paper` are unchanged by the bigger mirror, which is the
point of the layout: a package is fetched when the engine asks for it by
name, so a mirror ten times the size costs a document that asks for
nothing new exactly nothing. (`paper` reads lower than the 0.26 MB in
the table above only because it now runs after `article` in the same
browser and shares its cache; the two rows are not a like-for-like pair.)

## The fifth document, and what it could not use

`examples/latex/packages/` asks for the four packages an ordinary paper
asks for and the corpus does not. Two compile and two do not, for two
quite different reasons.

| package | collection | on SwiftLaTeX pdfTeX |
| --- | --- | --- |
| `booktabs` | latexrecommended | works; 1 page |
| `siunitx` | mathscience | mirrored and fetched, then **refuses** |
| `tikz` (`pgf`) | **pictures** | not mirrored: `tikz.sty` not found |
| `biblatex` | **bibtexextra** | not mirrored: `biblatex.sty` not found |

Two of the four are not in the four collections the step named at all.
`pgf` is in `collection-pictures` and `biblatex` in
`collection-bibtexextra`, and no amount of mirroring the named four will
produce them. Adding those two collections would cost a further **1550
files and 54.0 MB** for `pictures` and **1470 files and 19.3 MB** for
`bibtexextra`, over and above the 190.1 MB, and a paper with a drawn
figure or a modern bibliography needs both. That is a decision for
whoever takes step 3, with these numbers in hand.

`siunitx` is the more interesting failure, because the mirror did its
job: the engine asked for `siunitx.sty`, got it, loaded it, and then
stopped with

    ! Package siunitx Error: LaTeX kernel too old.

SwiftLaTeX's pdfTeX ships a preloaded format built from LaTeX2e
2020-02-02, and a 2025 `siunitx` will not run on a 2020 kernel. No
package set fixes that; only a rebuilt `.fmt` does, which is a question
about the distribution and not about the mirror. It is the same shape of
finding as BusyTeX's missing Type 1 EC fonts recorded above: what the
mirror carries and what the engine can use are two different questions,
and both have to be measured.

Nothing here was uploaded anywhere. The bytes are the report; the bucket
is somebody's decision.
