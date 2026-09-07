# The LaTeX mirror

`latex/mirror/` is what a browser fetches to compile a document: WasmTex's
engines and a pinned TeX Live package snapshot, mirrored under Komodoc's own
names so the browser never depends on an upstream project's live service.
The shape is the contract in
[`docs/specs/wasmtex-interfaces.md`](../../docs/specs/wasmtex-interfaces.md)
section 1; this file is how to build it and what is and is not verified yet.

## What is pinned where

- `latex/tools/wasmtex.mjs` names the pinned inputs at the top of the file:
  `ENGINE_RELEASE` (`2026-8b7946970153c52e`, the WasmTex engine release),
  `SNAPSHOT` (`2026-ba38749b8714505a`, the TeX Live package snapshot) and
  `WRAPPER_REVISION` (the WasmTex source revision the release was evaluated
  against, `44c5861fcdf729838205b00b96ac9509bc7fb677`). Changing any of these
  is a deliberate re-pin -- a new evaluation, a new manifest entry beside the
  old one, never an automatic follow of upstream.
- The evaluation record -- byte counts and SHA-256 for every engine file --
  lives at
  `latex/benchmark/candidates/wasmtex/downloads/manifest-2026.json`, produced
  by the comparison described in
  `latex/benchmark/candidates/comparison/README.md`. `wasmtex.mjs` refuses to
  mirror if the *live* upstream manifest's `releaseId` disagrees with the
  pinned one, or if any fetched file's bytes/sha256 disagree with this
  record.
- Licence notices (`LICENSE`, `THIRD_PARTY_NOTICES.md`, `docs/licensing.md`,
  `docs/corresponding-source.md`) and, for `wasmtex-record.mjs`, the host-side
  driver classes (`lib/engine/*.js`) are read from the WasmTex source
  checkout at `latex/benchmark/candidates/wasmtex/source` -- untracked,
  build-time input, not something either script writes to. Check out
  `WRAPPER_REVISION` there before running either script; a missing checkout
  fails loudly rather than silently skipping notices a release must carry.

## Building the mirror

```sh
# 1. The engine release: fetches, verifies against the pinned digests, and
#    writes latex/mirror/wasmtex/<engineRelease>/ plus the `releases` entry.
node latex/tools/wasmtex.mjs

# 2a. Package files, by key, fetched from the pinned TeX Live snapshot:
node latex/tools/wasmtex.mjs --texlive pdftex/26/amsmath.sty pdftex/26/article.cls

# 2b. Or from a JSON file: an array of keys, or an object whose values are
#     arrays of keys (the shape wasmtex-record.mjs's per-document tallies are
#     in, so the two compose).
node latex/tools/wasmtex.mjs --texlive-from corpus-keys.json

# 2c. Or by TeX Live collection, the way a distribution chooses a package
#     set: the TLPDB's file lists for basic, latex, latexrecommended,
#     latexextra, fontsrecommended and mathscience (the default), fetched
#     from the pinned snapshot sixteen at a time. This is what makes a
#     document outside the corpus compile; a name the snapshot lacks is
#     recorded absent. About 12,000 files and 270 MB.
node latex/tools/wasmtex.mjs --scheme

# 2d. Files the workers ask for at the snapshot's root rather than under an
#     engine/format pair: XeTeX's ICU data.
node latex/tools/wasmtex.mjs --texlive-root icudt68l.dat

# 3. The compact initial set the browser prefetches in parallel before a
#    first compile. Every key must already be fetched; this never fetches.
node latex/tools/wasmtex.mjs --initial pdftex/26/amsmath.sty pdftex/26/article.cls ...

# 4. The Biber VM release, built by latex/tools/biber-vm/build.mjs (Docker),
#    registered on the default release so the browser can find it.
node latex/tools/wasmtex.mjs --vm latex/mirror/biber-vm/<vmRelease>
```

Both fetch forms are idempotent -- a key already present, or already recorded
absent, is not asked for again -- so re-running costs a manifest read per
key, not a network round trip. Every `--texlive`/`--texlive-from` run
regenerates `texlive/<snapshot>/bloom-filter.v2.bin` over every key now
recorded present, and self-tests it (every present key must test positive
against the freshly built bytes; see `latex/tools/bloom.mjs`) before writing
it -- a bloom filter with a false negative would make the browser treat a
real file as absent forever.

## Recording the package set from real compiles

Steps 2a/2b above need to know which keys a document actually needs, and
that is not a list anyone should hand-type. `latex/tools/wasmtex-record.mjs`
gets it by actually compiling the corpus, in headless Chromium, against the
real WasmTex engines pointed at our own mirror server instead of upstream:

```sh
node latex/tools/wasmtex-record.mjs
```

It spawns `serve.mjs --record --lib <source checkout>/lib` (one origin,
because a Worker's script must be same-origin as the page that creates it),
drives Chromium via `web/tools/browser-driver.mjs` against that server's own
harness page, and compiles each of `latex/corpus/{article,paper,packages}`
with pdfTeX, `latex/corpus/xetex` and `latex/benchmark/fixtures/unicode-fonts`
with XeTeX, and a small inline fontspec document with LuaTeX -- TeX -> BibTeX
-> TeX -> TeX for the two documents with a plain BibTeX bibliography
(`article`'s is written by `\begin{filecontents*}` mid-compile and read back
out of the engine's own filesystem; `paper`'s is an ordinary `.bib` file).
`packages` uses biblatex/Biber, which WasmTex has no full backend for
(docs/specs/wasmtex.md), so it is compiled for its package requests only --
undefined citations in its log are expected, not a recording failure.

The wider corpus this tool was asked to cover --
`acm-conference`/`biber-related`/`biber-sorting`/`multifile` (and a real
`thesis` LuaTeX document) under
`latex/benchmark/.cache/projects/` -- is fetched by
`latex/benchmark/prepare.mjs` into a cache this checkout did not have
populated when this tool was written and run. `wasmtex-record.mjs` notices
and reports when that cache exists, but does not yet compile those cases; see
"What is left undone" below.

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

The manifest's `releases.<id>.source.reproduced` is `false`, and it should
stay that way until someone actually does the second half of "Own the
WasmTex release": rebuilding pdfTeX and BibTeX from source independently,
not merely mirroring verified upstream binaries (which is all this tool
does). The upstream build receipts (`BUILD-RECEIPT.<family>.json`, mirrored
into each release directory) and
`latex/benchmark/candidates/wasmtex/source/docs/corresponding-source.md`
describe the recipe:

```sh
# Illustrative -- the exact commands the upstream Docker build recipe uses,
# pinned by digest to the base image and Emscripten/TeX Live commits each
# BUILD-RECEIPT.<family>.json names. NOT run by this checkout: it takes
# hours, needs the pinned Docker base fetched, and its output has not been
# diffed against the mirrored binaries above. Read the actual recipe in the
# source checkout (wasm-build/, and the receipts' `buildId`/`sourceRevision`
# fields) before running it for real.
docker build -f wasm-build/Dockerfile.pdftex-bibtex \
  --build-arg TEXLIVE_COMMIT=<pinned in BUILD-RECEIPT.pdftex.json> \
  --build-arg EMSCRIPTEN_COMMIT=<pinned in BUILD-RECEIPT.pdftex.json> \
  -t wasmtex-pdftex-build .
docker run --rm -v "$PWD/out:/out" wasmtex-pdftex-build
```

This was deliberately not attempted here -- it takes hours of build time this
task's budget did not have, and "not attempted" must not read as "done and
skipped only in the log". Anyone who does run it should diff the result
against `wasmtex-pdftex.wasm`/`wasmtex-pdftex.fmt`/`wasmtex-bibtex.wasm` in
the mirrored release, and only then flip `reproduced` to `true`.

## Recording run actually performed here

`node latex/tools/wasmtex-record.mjs` was run against this checkout's
corpus. Per-document result (`tex1`/`bibtex`/`tex2`/`tex3` are compile
passes; `ok`/`FAIL` from the engine's own `success` flag):

| Document | Engine | Result |
| --- | --- | --- |
| `article` | pdfTeX | `tex1=ok bibtex=ok tex2=ok tex3=ok` -- full BibTeX round trip, including the `\begin{filecontents*}`-written `article.bib` read back out of the engine's own filesystem for BibTeX. |
| `paper` | pdfTeX | `tex1` fails: `pdfTeX error: pdflatex (file ./fig/one.png): reading image file failed`. Package/font resolution up to that point succeeded (fontenc, inputenc, amsmath, graphicx, natbib, komodoc.sty, the chapter file); the failure is pdfTeX's PNG decoder on that specific fixture, not a missing mirror file. |
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
- The wider corpus (`acm-conference`, `biber-related`, `biber-sorting`,
  `multifile`, a real `thesis` LuaTeX case) is not compiled: those cases live
  under `latex/benchmark/.cache/projects/`, populated by
  `latex/benchmark/prepare.mjs`, and this tool does not call that (outside
  this package's file allowlist to write, and not run here).
- Engine reproduction from source, per the "Reproduction status" section
  above.
- `manifest.releases.<id>.bibliography.biber.compatible` is inferred rather
  than independently verified: the mirrored snapshot's `biblatex.sty` is
  3.22 (2026-08-13) with control file version 3.11, unchanged from the 3.21
  this pinning's comparison evaluated against Biber 2.21 -- so the same
  Biber build is expected compatible, but that is an inference from the
  control-file version, not a pairing independently confirmed for this exact
  biblatex point release. See `wasmtex.mjs`'s `bibliographyIdentity`.
