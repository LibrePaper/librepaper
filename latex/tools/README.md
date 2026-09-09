# LaTeX mirror tools

LibrePaper consumes engine releases from `wasm-latex`. The engine repository
owns compilation, format generation, notices, and its release gate; this
repository owns the deployment mirror and the TeX Live package set.

## Building the mirror

In the wasm-latex repository, stage and check a release:

```sh
node tools/stage-release.mjs --dist wasm-build/dist --out staged --source-url <published-source-archive-URL>
node tools/check-release.mjs --dir staged
```

Successful staging prints the SHA-256 of `MANIFEST.json`. Review that digest and
pass it explicitly to LibrePaper:

```sh
make latex-mirror LATEX_RELEASE=../wasm-latex/staged LATEX_RELEASE_SHA256=<manifest-sha256>
# Or import just the engines:
node latex/tools/wasmtex.mjs --release ../wasm-latex/staged --sha256 <manifest-sha256>
```

A downloaded release directory works the same way; no source checkout is
required. There is no default release digest because the new
module has not published a complete release yet. Staging without passing its
release gate remains inspectable but cannot be imported.

The importer verifies the pinned manifest and every payload file before
writing the release, including notices and receipts. The manifest digest names
the mirror directory, preventing releases from overwriting one another. Only
complete engine sets are advertised: a pdfTeX/BibTeX release does not advertise
XeTeX or LuaTeX. Existing mirrors remain readable.

TeX Live packages still use the pinned `2026-ba38749b8714505a` snapshot. Package
mirroring is separate from engine building:

```sh
node latex/tools/wasmtex.mjs --scheme
node latex/tools/wasmtex.mjs --texlive pdftex/26/amsmath.sty pdftex/26/article.cls
node latex/tools/wasmtex.mjs --texlive-from corpus-keys.json
node latex/tools/wasmtex.mjs --texlive-root icudt68l.dat
node latex/tools/wasmtex.mjs --initial pdftex/26/amsmath.sty pdftex/26/article.cls
node latex/tools/wasmtex.mjs --vm latex/mirror/biber-vm/<vmRelease>
```

The package commands regenerate and verify the lookup filter. Initial-package
and VM registration apply to the current default release. Run the release
import first. The browser controller and local/VM fallback remain application
code.

## Recording the package set from real compiles

Steps 2a/2b above need to know which keys a document actually needs, and
that is not a list anyone should hand-type. `latex/tools/wasmtex-record.mjs`
gets it by actually compiling the corpus, in headless Chromium, against the
real WasmTex engines pointed at our own mirror server instead of upstream:

```sh
node latex/tools/wasmtex-record.mjs --lib <wasmtex checkout>/lib
```

`--lib` (or `WASMTEX_LIB`) names the `lib/` directory of a WasmTex source
checkout at the pinned revision; this repository does not track one.
It spawns `serve.mjs --record --lib <that directory>` (one origin,
because a Worker's script must be same-origin as the page that creates it),
drives Chromium via `web/tools/browser-driver.mjs` against that server's own
harness page, and compiles each of `latex/corpus/{article,paper,packages}`
with pdfTeX, `latex/corpus/xetex` and `latex/corpus/unicode-fonts`
with XeTeX, and a small inline fontspec document with LuaTeX -- TeX -> BibTeX
-> TeX -> TeX for the two documents with a plain BibTeX bibliography
(`article`'s is written by `\begin{filecontents*}` mid-compile and read back
out of the engine's own filesystem; `paper`'s is an ordinary `.bib` file).
`packages` uses biblatex/Biber, which WasmTex has no full backend for
(docs/specs/wasmtex.md), so it is compiled for its package requests only --
undefined citations in its log are expected, not a recording failure.

On completion it prints a per-document summary (`tex1=ok bibtex=ok
tex2=ok tex3=ok`, or `FAILED: <error>`), sets `manifest.texlive[<snapshot>]
.initial` to the union of keys the pdfTeX `article`/`paper`/`packages`
documents needed (via `wasmtex.mjs --initial`), and reports the mirror's new
size. Every package file it touches goes through the exact same
`ensureTexlive`-shaped path `wasmtex.mjs --texlive` writes -- digest-named,
recorded present or absent, bloom filter rebuilt once at the server's
shutdown rather than after every file (a compile can touch hundreds of
names in one run).

## Reproduction status

Build reproduction and source receipts belong to the wasm-latex repository.
The mirror retains those receipts and the source URL from its staged release.
It leaves `source.reproduced` false: importing bytes is not a reproduction
check.

The recording workflow above and the run history below describe the legacy
upstream SDK harness. It still needs a WasmTex source checkout for recording;
normal mirror construction via `make latex-mirror` does not use it.

## Recording run actually performed here

`node latex/tools/wasmtex-record.mjs` was run against this checkout's
corpus. Per-document result (`tex1`/`bibtex`/`tex2`/`tex3` are compile
passes; `ok`/`FAIL` from the engine's own `success` flag):

| Document | Engine | Result |
| --- | --- | --- |
| `article` | pdfTeX | `tex1=ok bibtex=ok tex2=ok tex3=ok` -- full BibTeX round trip, including the `\begin{filecontents*}`-written `article.bib` read back out of the engine's own filesystem for BibTeX. |
| `paper` | pdfTeX | `tex1` fails: `pdfTeX error: pdflatex (file ./fig/one.png): reading image file failed`. Package/font resolution up to that point succeeded (fontenc, inputenc, amsmath, graphicx, natbib, librepaper.sty, the chapter file); the failure is pdfTeX's PNG decoder on that specific fixture, not a missing mirror file. |
| `packages` | pdfTeX | `tex1` fails: `LaTeX Error: File 'pgfcorequick.code.tex' not found` after successfully resolving siunitx, booktabs and a large first slice of TikZ/pgf. Some part of the pgf core (or its upstream availability under this snapshot) was not reached before the fatal stop; biber-backed citations were never going to resolve here regardless (WasmTex has no full Biber backend), so this case's value is entirely in the package set it touches before failing. |
| `xetex`, `unicode-fonts` | XeTeX | Both fail at `internal error; cannot read font names`, preceded by `[icu] data unavailable (font-by-name will fail)`. XeTeX needs ICU data (docs/specs/wasmtex.md's "Required support data such as ICU") that this run never fetched or mirrored -- **not attempted**, not a resolver bug. |
| `luatex-mini` (inline fontspec doc, no thesis fixture was available) | LuaTeX | Timed out (180 s) without finishing; not diagnosed further. |

The mirror was still populated by every document, including the ones that
did not finish: `manifest.texlive["2026-ba38749b8714505a"]` has 188 files
recorded present and 116 recorded absent (20 MB on disk under
`latex/mirror/texlive/`), and `initial` (the pdfTeX `article`/`paper`/
`packages` union) is 89 keys, 15.9 MB -- from `paper` and `packages` this is
a lower bound of what those two actually need, since both stopped before
their fatal error would have surfaced more requests.

## What is left undone

- ICU data for XeTeX was never identified, fetched or mirrored; both XeTeX
  cases fail on it. This is required support data per docs/specs/wasmtex.md and
  belongs in the mirror layout (probably its own top-level entry, parallel
  to `wasmtex/` and `texlive/`) -- not designed here.
- `packages`' pgf/TikZ failure and `paper`'s PNG decode failure are
  unresolved; the former may just need `wasmtex-record.mjs` to retry a
  compile that failed midway (the log shows dozens of pgf files already
  resolved before the one that was not), the latter looks like a pdfTeX/PNG
  library issue independent of the mirror.
- `luatex-mini`'s timeout was not diagnosed; LuaTeX was never confirmed
  working end-to-end against this mirror.
- Engine reproduction from source, per the "Reproduction status" section
  above.
- `manifest.releases.<id>.bibliography.biber.compatible` is inferred rather
  than independently verified: the mirrored snapshot's `biblatex.sty` is
  3.22 (2026-08-13) with control file version 3.11, unchanged from the 3.21
  this pinning's comparison evaluated against Biber 2.21 -- so the same
  Biber build is expected compatible, but that is an inference from the
  control-file version, not a pairing independently confirmed for this exact
  biblatex point release. See `wasmtex.mjs`'s `bibliographyIdentity`.
