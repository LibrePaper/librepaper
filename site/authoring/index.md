---
title: "Authoring"
---

LibrePaper accepts several source formats. The pages in this section cover [LaTeX](./latex.md), [Typst](./typst.md), and [Quarto](./quarto.md), as well as common concerns like bibliographies and mathematical notation.

## Bibliographies

Add or upload a BibTeX or BibLaTeX file through Files, or include it when publishing
a directory. In the source editor, type `@` (or `\cite{` in LaTeX) to search by
citation key, author, title, or year. Selecting a result inserts its key.

Markdown accepts Pandoc citations such as `[@smith2020]`, `@smith2020`,
and `[see @smith2020, pp. 3-4; @jones2021]`. Choose resources and a built-in
citation style in front matter:

```yaml
---
bibliography: references.bib
bibliography-style: apa
---
```

With no resource declaration, all `.bib` files in the document form the library.
The reference list appears at a References heading or at the end. Missing keys,
malformed entries, and missing files appear in Diagnostics. Parsing and Markdown
formatting run in separate, lazily loaded WebAssembly modules. LaTeX and Typst
keep their own bibliography compilers. Zotero exports can be uploaded as
`.bib` files; live Zotero integration is a later milestone.

## Math

Markdown accepts TeX between dollars: `$\hat\beta$` in a sentence, and
`$$…$$` on lines of its own for a displayed equation. The renderer keeps the
TeX exactly as written, and the reader typesets it with KaTeX, served by the
deployment itself and fetched only by a document that has math in it. A
dollar with a number after it, as in `$5`, is still money. Typst and LaTeX
typeset their own mathematics.
