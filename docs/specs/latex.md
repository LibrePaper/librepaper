# SPEC: LaTeX in LibrePaper

The compiler, its resources, the local fallback and the browser Biber VM are
specified in [wasmtex.md](wasmtex.md); the module boundaries
that implement it are in [wasmtex-interfaces.md](wasmtex-interfaces.md). This
page is the short account of what a person sees, and of what remains.

## What happens when a LaTeX document opens

A reader sees the stored PDF at once and downloads no compiler. An editor's
browser loads LibrePaper's own pinned WasmTex release from the deployment's
`/latex/` mirror -- the engine the project needs and nothing else -- and
compiles automatically after the source has been quiet for a second and a
half, or at once from Compile now. There is no distribution to choose and no
bundle to download by hand. A release built by wasm-latex ships TeX Live as
one tar per package directory, indexed by `bundles.json`; the pdfTeX worker
fetches a whole package the first time any file in it is asked for, verifies
its digest, and keeps it in Cache Storage, so a warm session makes no
requests. The rule for what is bundled, and why, is wasm-latex's
`SPEC-latex.md`. Releases without bundles, and the other engines, still
receive packages one file at a time, with the ones every document needs
arriving together before the first pass.

BibTeX runs in the browser. Biber does not: when a document asks for it, the
reader checks for a local LibrePaper app on this machine, runs a compatible
native Biber there, and continues typesetting in the browser. Without the
app, or without a compatible Biber, the reader boots a small Linux guest in a
worker -- v86 and a Debian image holding Biber and nothing else -- and runs
the real Biber there. It is slower than native and needs no installation.

When browser compilation itself fails -- an engine that will not initialise, a
resource the mirror lacks, a crash, a timeout, or a TeX error -- the reader
asks the local app to compile the whole project natively, once per snapshot.
A success is an ordinary preview with local provenance; the browser attempt's
log stays in Diagnostics. A project stays on the native route for the rest of
the session, with "Try browser compilation" to come back.

## What the local app is

`librepaper local start` runs a loopback service on this machine, prints a
pairing code, and waits. The reader connects with that code once per origin
and project; later fallbacks are automatic. `librepaper local doctor` says which
of pdfLaTeX, XeLaTeX, LuaLaTeX, BibTeX, Biber and makeindex it found, at
which versions, and whether it can confine them; `librepaper local status` and
`librepaper local disconnect` do what their names say. The service accepts
structured jobs -- a snapshot's files with their digests, an engine name, a
job name -- and never a command. It runs the tools with shell escape off,
inside `bwrap` or `sandbox-exec` where the platform has them, and reports
plainly when it cannot confine them. It installs nothing.

## Settings

The Settings panel of a LaTeX document has four things: the project engine
(Automatic, pdfLaTeX, XeLaTeX, LuaLaTeX), the pinned browser release with an
explicit update, the local connection with its status and controls, and the
compiler cache with its size and a clear. The engine and the release are
project settings, kept in the shared document and part of every checkpoint's
identity, so a change of engine can never reuse a PDF compiled under another.
The local connection is this device's alone.

## SyncTeX

The browser engines write SyncTeX with every pass and the reader stores the
`.synctex.gz` beside the PDF it came from, never one from a different job.
The viewer's two gestures -- the caret's line scrolls the page to its box,
and a double-click on the page moves the caret to the line -- read that file.

## What was verified

`web/tools/latex-e2e.mjs` drives the routing table against the built binary
in headless Chromium: the corpus compiles in the browser with citations and
SyncTeX; a biblatex project gets its bibliography from a paired local app in
about five seconds, and from the browser VM in a minute or two on a cold
cache; a project needing a package the mirror lacks is compiled natively by
the paired app and stays on that route until "Try browser compilation".
XeTeX and LuaTeX documents compile in the browser.

## Remaining

- The WasmTex XeTeX core writes no SyncTeX, so a XeLaTeX document has no
  source mapping until the engine is rebuilt with it; pdfTeX and LuaTeX do.
- Reproducing every engine from source. The wasm-latex repository rebuilds
  pdfTeX and BibTeX byte-for-byte from pinned TeX Live sources; XeTeX,
  LuaTeX, bibtex8 and makeindex are still the mirrored upstream build, and
  the manifest says `reproduced: false` until all of them are.
- The compact initial resource set is the union of what the corpus needed
  (about 17 MB); measuring real documents should trim it. With a bundled
  release it is replaced by the `core` bundle, 32 MB, which the same
  measurement should trim.
- A XeLaTeX document has not yet been compiled in a browser against a
  bundled release -- only in the Node harness on the engine side. The
  LuaTeX worker does not resolve through bundles yet (it will once its
  release ships `wasmtex-kpse-resolve.js`/`wasmtex-bundle-mode.js` like the
  others), and a bundled release is not yet imported into the shipped
  mirror.
- Detaching `librepaper local start` from its terminal and registering the
  `librepaper://` protocol on each desktop platform.
- The acceptance matrices on Firefox and Safari and on memory-constrained
  devices; the numbers so far are single Chromium runs.
