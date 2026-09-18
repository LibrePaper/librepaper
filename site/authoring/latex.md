---
title: "LaTeX"
---

## LaTeX

Publishing a `.tex` file stores it as `latex`, and publishing a directory takes
the whole project: the chapters, the `.bib`, the figures. Nothing is compiled
on the way: LibrePaper carries no TeX, and no build embeds one. See
[the CLI](../cli.html#publish).

LaTeX is compiled in the browser, by LibrePaper's own pinned release of the
browser engines: pdfTeX, XeTeX and BibTeX built for WebAssembly, with
the formats generated for those exact binaries and a pinned TeX Live package
set. An editor's browser fetches the current release from the configured HTTPS
mirror and compiles automatically; readers render the source on demand.
Packages arrive as verified, content-addressed bundles from that mirror and
stay in browser storage so the next document costs nothing to fetch. The TeX engines carry
their own licences, and Biber is AGPL-3.0. They are fetched at run time;
their notices travel with the mirror.

In the editor, **View → Preview format → HTML** selects a live LaTeXML
preview; **PDF** returns to the printed layout. The choice is remembered in
this browser for this document. HTML conversion runs in a separate WebAssembly
worker and reuses the mirror's verified TeX package bundles. It requires a
mirror release containing the `latexml` engine, built and hosted by
[`wasm-latex`](https://github.com/LibrePaper/wasm-latex). The app contains only
the adapter and preview controls. HTML is a transient reading view. An
unsuccessful edit may leave the current preview visible while reporting
conversion diagnostics; it does not create a stored rendering.

The project engine (Automatic, pdfLaTeX, XeLaTeX or LuaLaTeX) is a project
setting in the Settings dialog. Every compile uses the mirror's current
default release; documents do not pin a browser release. Automatic honours
a `% !TEX program = xelatex` line in the main file, then looks for packages
that only a Unicode engine can load, and otherwise uses pdfLaTeX. LuaLaTeX
remains in the selector for release compatibility, but selecting it with the
current release reports that it is not available in this release.

BibTeX and Biber run in the browser when the release provides them. Biber
documents use the release's bundled biblatex pairing. If browser Biber has an
infrastructure failure, the reader can hand the `.bcf` and `.bib` files to the
local companion and continue typesetting in the browser. Bibliography input
errors are shown directly and are not retried through another backend. If the
companion is unavailable, the reader explains that local Biber is required.

The local companion extends the online editor with the tools installed on
your computer. Documents and collaboration stay in the website. Install the
companion from the document's **Enable local rendering** settings, then use
**Open companion** to launch it. The first connection asks permission for
the named site and document; subsequent connections reuse that permission,
including after restarting the companion.

The companion's local settings page shows discovered tools and connected
documents, lets you revoke access, and offers **Start at login** and **Quit
companion**. Startup at login is optional. A browser may separately ask for
permission to connect to a local service; allow that for the LibrePaper site
you use. Compilation permissions do not grant the website access to these
local management controls.

The companion is the same binary as the CLI, and can be run and inspected from
a terminal instead; see [the companion](../cli.html#the-companion).

Enter the pairing code once in the document's Settings dialog and later fallbacks are
automatic. When browser compilation fails outright (an engine that will not
start, a package the mirror lacks, a crash, a TeX error), the reader asks the
app to compile the whole project natively with your installed TeX, once per
version of the source, and shows the result as an ordinary preview that says
it was made locally. The app accepts structured jobs rather than commands,
runs the tools with shell escape off, confines them with `bwrap` or
`sandbox-exec` where the platform has them, and says so when it cannot. It
never installs packages or changes your TeX installation.

A self-hoster can serve the browser distribution from their own mirror rather
than the project one; see
[Privacy and the LaTeX mirror](../host.html#privacy-and-the-latex-mirror).

`make deploy` checks that the selected mirror contains a default engine
release with its TeX Live bundles (`tools/latex/tools/check-mirror.mjs`; see
`make latex-check` and `make latex-smoke`, MIRROR=). Older per-file mirrors
and SwiftLaTeX/BusyTeX releases are rejected as legacy. `make latex-smoke` compiles `docs/examples/tutorial-latex/librepaper.tex`
in a fresh Chromium profile against MIRROR= and requires visible PDF pages
and selectable text before you point a deployment at it.

LibrePaper always serves the LaTeX editor and compiler configuration. Browsers
fetch distribution files directly from the HTTPS mirror; the origin does not
proxy or cache them. A directory path or an `http:` mirror is refused at
startup. The browser verifies each file against the mirror manifest before
use.
