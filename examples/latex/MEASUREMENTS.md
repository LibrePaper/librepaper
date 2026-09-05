# The distributions, measured

Written by `node web/scripts/latex-check.mjs --measure` against the local
mirror `node latex/mirror.mjs` builds, in headless Chromium, on one machine.
Bytes are what the mirror actually served, counted by `latex/serve.mjs`, with
Cache Storage emptied before each distribution and the byte tally reset before
each compile -- so a document's row is what a cold cache costs for that
document alone, over and above the up-front row.

| distribution | what | bytes fetched | cold | warm | pages |
| --- | --- | --- | --- | --- | --- |
| swiftlatex-pdftex | up front | 1.96 MB | 0.0 s | -- | -- |
| swiftlatex-pdftex | article | 17.54 MB | 0.7 s | 0.2 s | 3 |
| swiftlatex-pdftex | paper | 0.10 MB | 0.3 s | 0.2 s | 4 |
| swiftlatex-pdftex | broken | 0.00 MB | 0.1 s | 0.1 s | 0 |
| swiftlatex-pdftex | xetex | 0.09 MB | 0.0 s | 0.0 s | 0 |
| swiftlatex-xetex | up front | 3.98 MB | 0.0 s | -- | -- |
| swiftlatex-xetex | article | 29.54 MB | 0.7 s | 0.2 s | 3 |
| swiftlatex-xetex | paper | 0.10 MB | 0.3 s | 0.2 s | 4 |
| swiftlatex-xetex | broken | 0.00 MB | 0.0 s | 0.0 s | 0 |
| swiftlatex-xetex | xetex | 1.01 MB | 0.5 s | 0.3 s | 0 |
| busytex | up front | 4.78 MB | 0.1 s | -- | -- |
| busytex | article | 182.73 MB | 3.0 s | 1.2 s | 0 |
| busytex | paper | 0.00 MB | 1.2 s | 1.2 s | 0 |
| busytex | broken | 0.00 MB | 0.5 s | 0.4 s | 0 |
| busytex | xetex | 0.00 MB | 1.2 s | 1.1 s | 1 |
| texlyre-busytex | up front | 46.22 MB | 0.1 s | -- | -- |
| texlyre-busytex | article | 635.55 MB | 4.4 s | 0.8 s | 3 |
| texlyre-busytex | paper | 0.00 MB | 0.9 s | 0.9 s | 4 |
| texlyre-busytex | broken | 0.00 MB | 0.5 s | 0.4 s | 0 |
| texlyre-busytex | xetex | 0.00 MB | 2.4 s | 2.3 s | 1 |

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

# TeXlyre BusyTeX, measured

Written by hand from `node latex/mirror.mjs --only texlyre-busytex` and a
headless Chromium compile against the mirror it built, on one machine, the
same way and on the same corpus as the sections above -- plus the fifth
document, `examples/latex/packages/`, which the harness does not carry and
which was driven through the same module by a scratch script.

`texlyre-busytex` is TeXlyre's own build of BusyTeX: TeX Live **2026**,
pdfTeX, XeTeX and LuaHBTeX in one module, bibtex8, makeindex, a standalone
biber wasm, a JavaScript shell-escape hook, and `-synctex=1` on every engine
invocation. It is AGPL-3.0, derived from BusyTeX, which is MIT. It is pinned
to release `assets-v1.4.0`, a single 521 MB `busytex-assets.tar.gz`. The npm
package of the same name carries only a TypeScript driver -- no wasm, no data
-- and none of it is used: `web/src/lib/latex/texlyre.js` is written against
the pipeline the tarball ships, as `busytex.js` is written against BusyTeX's.

It can be mirrored two ways, and the difference between them is the whole
question. Either the two further TeX Live data bundles are mirrored and
offered as whole blobs, as BusyTeX's are; or none of them is, and the engine
fetches one file at a time from a package endpoint whose URL shape --
`<base>/<kpathsea format>/<name>` -- is SwiftLaTeX's protocol without the
engine segment, so the package half of this mirror already answers it
unchanged. Both were measured.

| distribution | what | bytes fetched | cold | warm | pages |
| --- | --- | --- | --- | --- | --- |
| texlyre-busytex (bundles) | up front | 46.22 MB | 0.1 s | -- | -- |
| texlyre-busytex (bundles) | article | 635.55 MB | 4.4 s | 0.8 s | 3 |
| texlyre-busytex (bundles) | paper | 0.00 MB | 0.9 s | 0.9 s | 4 |
| texlyre-busytex (bundles) | broken | 0.00 MB | 0.5 s | 0.4 s | 0 |
| texlyre-busytex (bundles) | xetex | 0.00 MB | 2.4 s | 2.3 s | 1 |
| texlyre-busytex (bundles) | packages | 0.00 MB | 3.9 s | 2.9 s | 2 |
| texlyre-busytex (on demand) | up front | 34.99 MB | 0.1 s | -- | -- |
| texlyre-busytex (on demand) | article | 94.62 MB | 3.4 s | 0.8 s | 3 |
| texlyre-busytex (on demand) | paper | 0.00 MB | 0.8 s | 0.9 s | 4 |
| texlyre-busytex (on demand) | broken | 0.00 MB | 0.4 s | 0.4 s | 0 |
| texlyre-busytex (on demand) | xetex | 0.16 MB | 3.5 s | 0.9 s | 1 |
| texlyre-busytex (on demand) | packages | 2.24 MB | 16.9 s | 1.5 s | 0 -- see below |

The `bundles` rows are the four in the table at the top of this file, measured
by `latex-check.mjs --measure` in the same run as the other three
distributions; the `packages` row and every `on demand` row are from the
scratch script. Read them the same way: a document's row is what a cold Cache
Storage costs for that document alone, over and above the up-front row.

**What the card number would be.** Up front plus the first document:
**682 MB** with the bundles mirrored, **130 MB** with the endpoint. Neither is
the 128 MB the mirror calls "up front", because the pipeline constructs itself
before it has seen a document and pulls its TeX Live down when one arrives.

The 682 MB deserves its own sentence, because it is not what the design
promises. The pipeline resolves the packages a document names against the
`\ProvidesPackage` lines in each bundle's loader and means to load only the
bundles that carry them -- but if any package is left unresolved, it gives up
and enables *every* bundle it was offered. `article` leaves one unresolved, so
it fetches all 586 MB, and the whole of TeX Live 2026 is in the browser before
the first page. Every document after it, including the four-package one, then
costs nothing. That is "large to start and quiet afterwards" taken to its
limit, and 682 MB is past what a card can honestly ask an author to accept.

Say the rest of it plainly, because the size is the symptom and not the
illness: what makes this distribution unshippable today is the **failure
mode**, not the 682 MB. A mirror that answered everything would cost 130 MB a
session, and 130 MB is a number a card could carry. But the fallback is
all-or-nothing and its trigger is one unresolved name, so a single missing
`.cfg` -- one file, out of the ten thousand in the package half of this
mirror -- silently turns that 130 MB session into a 682 MB one, with nothing
said to the reader and no way for the card to have promised the smaller figure
honestly. A cost that degrades by a factor of five on a condition the author
can neither see nor predict is not a cost a card can state at all. Fix the
trigger, or make the fallback incremental so an unresolved name pulls one
bundle rather than every bundle, and the size question becomes an ordinary
one to be answered with the numbers above.

The endpoint mode is the interesting one. 130 MB before the first page, of
which 93 MB is `texlive-basic.data`, and then a sixth of a megabyte for the
XeTeX document and nothing at all for the second paper -- the same
small-and-chatty shape SwiftLaTeX has, on a 2026 TeX Live, with the engine
building its own URLs against our mirror and no package name reaching anyone
else.

**What it compiles that pdfTeX could not.** All four of the packages
`examples/latex/packages/` asks for, in one document, in two pages, matching a
TeX Live on a desk, with the bundles mirrored:

| package | on SwiftLaTeX pdfTeX | on texlyre-busytex (bundles) |
| --- | --- | --- |
| `booktabs` | works | works |
| `siunitx` | `! Package siunitx Error: LaTeX kernel too old.` | works |
| `tikz` | `! LaTeX Error: File 'tikz.sty' not found.` | works |
| `biblatex` | `! LaTeX Error: File 'biblatex.sty' not found.` | works |

`siunitx` is the one that matters, and it is the reason the 2026 build was
worth measuring at all: SwiftLaTeX's preloaded format is LaTeX2e 2020-02-02
and no package set fixes that, where a 2026 kernel simply runs it.
`\usepackage[T1]{fontenc}` works too -- `article` and `paper` are 3 and 4
pages, where BusyTeX's 2023 release has no Type 1 EC fonts in any bundle and
refuses both. And the `xetex/` example sets and embeds its font in one page,
where SwiftLaTeX's XeTeX stops at `Cannot proceed without the font`. Of the
five corpus documents, this is the only distribution of the four that
compiles every one of them.

**What it could not.** Two things.

`biber` does not work, in either mirroring mode, and the failure is in the
released pipeline rather than in this glue. It is written out here as a repro,
at length, because the README advertises biber as a headline feature and
because the next person to reach for it should find this before spending a day
on their own glue.

**Repro.** Mirror `texlyre-busytex` as above, choose it, and compile this
document -- nothing in it but `biblatex`, and no package from any bundle
except the one `biblatex` itself lives in:

    \documentclass{article}
    \begin{filecontents*}[overwrite]{r.bib}
    @article{k1984, author={Knuth, Donald E.}, title={Literate Programming},
             journal={Comp J}, year={1984}}
    \end{filecontents*}
    \usepackage[backend=biber]{biblatex}
    \addbibresource{r.bib}
    \begin{document}
    Hello \cite{k1984}.
    \printbibliography
    \end{document}

**What happens.** Four commands run, and the log records them in this order:

    $ pdflatex ... main.tex        EXITCODE: 0
    $ pdflatex ... main.tex        EXITCODE: 0
    (biber runs; its wasm loads, and it writes a .bbl)
    $ pdflatex ... main.tex        EXITCODE: 1

and the third pdfTeX pass ends at

    ! LaTeX Error: File `biblatex.sty' not found.
    ! Emergency stop.

The first two passes found `biblatex.sty` perfectly well. Between the second
and the third, the pdfTeX module has been reloaded and has lost *every*
on-demand data package it had -- so the package the document is about is gone,
and the pass that was to consume biber's output cannot start. Biber itself is
not the broken part: its module loads, it runs, and it produces a `.bbl`. What
breaks is the pdfTeX module underneath it.

The same defect on the four-package document is the same shape one step later:
it had `siunitx` a moment earlier and ends at
`! LaTeX Error: File 'siunitx.sty' not found.`

**What works instead.** `biblatex` with `backend=bibtex` -- which is what
`examples/latex/packages/` asks for -- compiles to its two pages through
bibtex8 without trouble. So `biblatex` the package works, and only the biber
backend does not. Until that is fixed, a document with `\addbibresource` and
no explicit backend must not be given biber silently: biblatex's own default
is biber, so the honest choices are to fix the pipeline or to tell the author
which backend they are actually getting.

In the endpoint mode, `packages` also fails, and there the fault is ours: the
engine asked for `biblatex-dm.cfg`, the package half of the mirror does not
have it, no TeX Live on this machine could supply it, and biblatex then died
at `! Package biblatex Error: Patching \MakeUppercase failed.` That mirror was
collected for SwiftLaTeX and covers no biblatex configuration files. It is the
same shape of finding as `siunitx` was for SwiftLaTeX in the section above --
what the mirror carries and what the engine can use are two different
questions -- and it is fixable by mirroring, unlike the biber one.

**SyncTeX.** It is real, and this is the first of the four distributions for
which that is true. `-synctex=1` is in every argument vector the pipeline
builds, and a run that produced a page produces a `.synctex.gz` beside it: 9445
bytes for `article`, 6982 for `paper`, 5221 for `packages`, 1794 for `xetex`,
and nothing for `broken`, which is correct. The bytes carry the gzip magic
`1f 8b`, gunzip cleanly, and begin

    SyncTeX Version:1
    Input:1:/home/web_user/project_dir/./main.tex
    Input:2:/texlive/texmf-dist/tex/latex/base/article.cls

so the document's own file is input 1 and the rest are the distribution's. The
paths are the engine's absolute paths inside its filesystem, not the tree's
paths, which is the one thing whoever builds `05-SPEC-latex.md` step 6 will
have to map. Step 6 was blocked on there being no SyncTeX anywhere; it is not
blocked any more.

**On disk.** The mirror holds 713.5 MB for this distribution alone -- 127.7 MB
up front (the 32.5 MB engine wasm and the 92.8 MB `texlive-basic.data`),
348.9 MB and 205.1 MB for the two further bundles, and 31.8 MB for biber --
plus the 521 MB release tarball under `.cache`, which a deployment does not
host. A deployment that offered the bundles would host 713.5 MB; one that
offered the endpoint instead would host 128 MB for this distribution and lean
on the package half of the mirror, which is 238.5 MB shared with SwiftLaTeX
and does not yet cover biblatex's configuration files. Nothing was uploaded
anywhere.

**Recommendation: not yet, and then beside pdfTeX rather than instead of it.**
It is the only distribution of the four that compiles the whole corpus and the
only one with SyncTeX, and `siunitx` and a 2026 kernel are exactly what
SwiftLaTeX cannot be given at any price -- but 682 MB before the first page is
not a number to put on a card, and the endpoint mode that brings it down to
130 MB is the mode whose mirror is incomplete and whose biber is broken. The
work between here and the card is small and known: mirror the biblatex and
pgf configuration files into the package half so the endpoint mode compiles
the fifth document, and either fix or disable the biber backend so a
`\addbibresource` document is told which backend it will get. Its entry in
`latex/distributions.mjs` carries `shown: false` until then.
