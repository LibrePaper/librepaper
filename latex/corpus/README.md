# The LaTeX corpus

Four documents, each a directory with a `main.tex`, written when LaTeX in the
browser was first specified. They exist to be compiled by every distribution
LibrePaper drives and by a TeX Live on a desk, and to disagree with none of
them. What each is for:

- **`article/`** — one file, with its bibliography inside it in
  `filecontents*`. It is the shape the spec was written against before a
  document became a directory, and it keeps that case working. It also carries, deliberately, the four things step 4's
  anchoring has to survive: a hyphenated line end, a page break inside a
  sentence, a footnote, and an `fi` ligature. Three pages.

- **`paper/`** — the ordinary paper the directories spec is for. A main
  file, two chapters reached by `\include`, a `refs.bib` beside them, a
  PNG and a PDF included by relative path, and a `librepaper.sty` of its
  own that no TeX Live has — so a compile fails outright unless every
  path of the tree reached the engine's filesystem. Four pages.

- **`broken/`** — four things wrong on purpose: an undefined control
  sequence in a chapter, a missing `\input`, an overfull hbox, and a
  package warning. It is the log parser's test bed. `expected.json`
  holds, per engine, exactly what the parser must say about the log that
  engine produced; the engines disagree, because what a TeX gets round to
  reporting before it stops is not the same on all of them. No pages.

- **`packages/`** — not part of the corpus the parser and the
  distributions are held to. It asks for `booktabs`, `siunitx`, `tikz`
  and `biblatex`, which is what an ordinary paper asks for and the four
  above do not, and it exists to measure a package set rather than a
  compiler. Two pages on a TeX Live; `MEASUREMENTS.md` records what it
  can and cannot do on SwiftLaTeX pdfTeX.

- **`xetex/`** — a document that needs XeTeX: `fontspec`, and Greek and
  Cyrillic in the source rather than as macros. It is how a pdfTeX-only
  distribution is shown refusing cleanly, with the error `fontspec`
  gives, rather than producing a wrong page. One page.

- **`unicode-fonts/`** — also XeTeX: named Libertinus text and math fonts
  through `fontspec` and `unicode-math`, with Greek and Cyrillic text. Not
  held to `pages.json`; wasm-latex's package recorder compiles it so the
  named-font and `unicode-math` package requests reach the mirror.

## What is generated, and by what

- `logs/<engine>.log` — the log each engine produced. `texlive.log` comes
  from `node latex/tools/texlive.mjs`; the rest from
  `node web/checks/latex.mjs --measure`. These are the parser's
  fixtures and `web/checks/latex-log.mjs` runs over all of them.
- `pages.json` — the page count a TeX Live on this machine gives each
  document, which every distribution is then held to.
- `broken/expected.json` — the diagnostics, per engine.
- `MEASUREMENTS.md` — bytes and seconds per distribution per document.

Nothing here is checked into the mirror and nothing here is uploaded
anywhere. `latex/mirror/` is a local directory, ignored by git.
