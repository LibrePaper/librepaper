# The LaTeX corpus

The corpus contains browser-comparison examples, an intentional negative
fixture, and independent local TeX Live probes. Every document has a directory
with a `main.tex`; each has a distinct purpose:

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
  compiler. Two pages on a TeX Live.

- **`xetex/`** — a document that needs XeTeX and `fontspec`. It is how a
  pdfTeX-only distribution is shown refusing cleanly, with the error
  `fontspec` gives, rather than producing a wrong page. It is an
  engine-admission and historical diagnostic fixture, not a Unicode success
  oracle: the retained native XeTeX log records missing Greek and Cyrillic
  glyphs because this sample deliberately names no font. Its recorded page
  count only shows that XeTeX emitted a PDF; it does not claim complete text
  rendering.

- **`unicode-fonts/`** — a positive XeTeX Unicode glyph-coverage sample:
  named Libertinus text and math fonts through `fontspec` and `unicode-math`,
  with Greek and Cyrillic. The TeX Live oracle requires a positive page count
  and rejects `Missing character` diagnostics. It is not run through the
  browser comparison because those workers do not promise system fonts; the
  wasm-latex package recorder also compiles it so the named-font and
  `unicode-math` package requests reach the mirror.

## What is generated, and by what

- `logs/<engine>.log` — the log each engine produced. `texlive.log` comes
  from `node tools/test/latex/texlive.mjs`; the rest from
  `node web/tests/browser/latex-browser.mjs --measure`. These are the parser's
  fixtures and `web/tests/unit/latex-log.mjs` runs over all of them.
- `pages.json` — the page count a TeX Live on this machine gives each
  browser-comparison document. The TeX Live oracle also checks
  `unicode-fonts` as a separate positive glyph-coverage sample; the browser
  harness compares only its explicit `article`, `paper`, `packages`, and
  `xetex` cases.
- `broken/expected.json` — the diagnostics, per engine.

Nothing here is checked into the mirror and nothing here is uploaded
anywhere. `tools/test/latex/mirror/` is a local directory, ignored by git.
