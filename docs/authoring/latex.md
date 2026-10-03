---
title: "LaTeX"
---

## LaTeX

Publishing a `.tex` file stores it as `latex`, and publishing a directory takes
the whole project: chapters, `.bib`, figures. Nothing is compiled on the way.

LaTeX compilation happens in the browser:
- Compiled by pinned WebAssembly releases: pdfTeX, XeTeX, BibTeX with TeX Live packages
- Packages fetched from the configured HTTPS mirror as verified, content-addressed bundles
- Cached in browser storage; subsequent documents cost nothing to fetch
- Browsers fetch distribution files directly from the mirror; the server does not proxy
- Mirror must be HTTPS; directory paths and `http:` mirrors are refused at startup

HTML preview via LaTeXML conversion is available. HTML conversion runs in a
separate WebAssembly worker and reuses the mirror's verified TeX package
bundles. It requires a mirror release containing the `latexml` engine, built
and hosted by [`wasm-latex`](https://github.com/LibrePaper/wasm-latex). HTML is a
transient reading view; an unsuccessful edit may leave the current preview
visible while reporting conversion diagnostics.

The project engine (Automatic, pdfLaTeX, XeLaTeX or LuaLaTeX) is configurable
per project. Every compile uses the release the server pins in `assets.lock`.
Automatic honours a `% !TEX program = xelatex` line in the main file, then
looks for packages that only a Unicode engine can load, and otherwise uses
pdfLaTeX. LuaLaTeX is listed for compatibility but
the current release does not provide it.

BibTeX and Biber run in the browser when the release provides them. Biber
documents use the release's bundled biblatex pairing. Bibliography input
errors are shown directly.

A self-hoster can serve the browser distribution from their own mirror rather
than the project one; see [Privacy](../host.html#privacy).
