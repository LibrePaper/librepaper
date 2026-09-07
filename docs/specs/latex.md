# SPEC: LaTeX in Komodoc

The compiler, its resources, the local fallback and the browser Biber VM are
specified in [SPEC-wasmtex.md](../../SPEC-wasmtex.md); the module boundaries
that implement it are in [wasmtex-interfaces.md](wasmtex-interfaces.md). This
page is the short account of what a person sees, and of what remains.

## What happens when a LaTeX document opens

A reader sees the stored PDF at once and downloads no compiler. An editor's
browser loads Komodoc's own pinned WasmTex release from the deployment's
`/latex/` mirror -- the engine the project needs and nothing else -- and
compiles automatically after the source has been quiet for a second and a
half, or at once from Compile now. There is no distribution to choose and no
bundle to download by hand: packages arrive one file at a time from the
mirror as a compile asks for them, and the ones every document needs arrive
together before the first pass. Verified files stay in browser storage,
namespaced by release, so the second document costs nothing to fetch.

BibTeX runs in the browser. Biber does not: when a document asks for it, the
reader checks for a local Komodoc app on this machine, runs a compatible
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

`komodoc local start` runs a loopback service on this machine, prints a
pairing code, and waits. The reader connects with that code once per origin
and project; later fallbacks are automatic. `komodoc local doctor` says which
of pdfLaTeX, XeLaTeX, LuaLaTeX, BibTeX, Biber and makeindex it found, at
which versions, and whether it can confine them; `komodoc local status` and
`komodoc local disconnect` do what their names say. The service accepts
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

## Remaining

- Reproducing the engine binaries and their formats from the pinned WasmTex
  sources, rather than mirroring the verified upstream build; the manifest
  says `reproduced: false` until that is done.
- Detaching `komodoc local start` from its terminal and registering the
  `komodoc://` protocol on each desktop platform.
- The acceptance matrices on Firefox and Safari and on memory-constrained
  devices; the numbers so far are single Chromium runs.
