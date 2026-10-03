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
documents use the release's bundled biblatex pairing. If browser Biber has an
infrastructure failure, the reader can hand the `.bcf` and `.bib` files to the
local companion and continue typesetting in the browser. Bibliography input
errors are shown directly and are not retried through another backend. If the
companion is unavailable, the reader explains that local Biber is required.

The local companion extends the online editor with tools installed on your
computer. Install from the [install page](../install.html), then start it:

```sh
librepaper                           # start in the background
librepaper --at-login                # also start every time you log in
```

The first connection asks permission for the named site and document;
subsequent connections reuse that permission, including after restarting the
companion. Compilation permissions do not grant the website access to local
management controls.

The companion is the same binary as the CLI; see [the companion](../cli.html#the-companion).

When browser compilation fails (an engine that will not start, a package the
mirror lacks, a crash, a TeX error), the reader falls back to compiling
natively with your installed TeX, once per version of the source. The app
accepts structured jobs rather than commands, runs the tools with shell escape
off, confines them with `bwrap` or `sandbox-exec` where available, and says so
when it cannot. It never installs packages or changes your TeX installation.

A self-hoster can serve the browser distribution from their own mirror rather
than the project one; see [Privacy](../host.html#privacy).
