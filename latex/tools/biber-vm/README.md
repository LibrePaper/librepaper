# Biber VM: browser bibliography fallback image

Builds the minimal 32-bit guest that runs real Biber inside a v86 emulator
worker, for [docs/specs/wasmtex.md](../../../docs/specs/wasmtex.md)'s "Browser Biber VM:
final bibliography fallback" and
[docs/specs/wasmtex-interfaces.md](../../../docs/specs/wasmtex-interfaces.md)
section 6. This is package F. It never runs on the ordinary browser
TeX/BibTeX path; it is a last-resort fallback for Biber only, loaded lazily.

## What is in the guest

- `debian:bookworm-slim` (i386), pinned by digest.
- One file: `/usr/local/bin/biber`, version **2.21**.
- Nothing else. No TeX, no Perl package, no fonts, no docs/man pages, no apt
  caches, no locales beyond `C.UTF-8` (provided by glibc itself, no
  `locales` package needed).

Biber's official TeX Live binary distribution turns out to be an ideal fit
for a minimal guest: it is a PAR::Packer (`pp`) self-contained executable
that bundles its own private Perl interpreter and every CPAN module it
needs. `ldd` inside an i386 `bookworm-slim` container shows it depends on
nothing but `libc.so.6`/`libpthread.so.0`/the vDSO/`ld-linux.so.2`, all
already present in the base image. No system Perl, no `fontconfig`, no `curl`,
no `ca-certificates` were installed.

One build-time gotcha: `biber --version` unpacks that bundled Perl runtime
into `$TMPDIR/par-<hash>/` (about 97 MB) on first run. Running it during the
Docker build to sanity-check installation, without also cleaning `/tmp` in
the *same* `RUN` layer, silently baked that cache into the image (observed:
75 MB image inflated the exported rootfs to 204 MB and the packed VM release
to 186 MB). The Dockerfile now does `... && biber --version && rm -rf /tmp/*`
in one layer. The guest still re-extracts this cache lazily on its own first
`biber` invocation at VM runtime; that is an unavoidable one-time cost per
boot, not per Biber call within one boot (see Results below).

## Pinning Biber to the browser release

The browser release's biblatex (`texlive.corca.ai` snapshot
`2026-ba38749b8714505a`, `pdftex/26/biblatex.sty`) defines
`\blx@bcfversion{3.11}`. That bcf format requires **Biber 2.21**, matching
`manifest.json`'s `releases.<id>.bibliography.biber.compatible = ["2.21"]`.
The developer machine's local TeX Live 2025 (nixos.org build) independently
produces the same `\blx@bcfversion{3.11}`, which `vm-smoke.mjs` uses as
cross-check evidence (see its bcf-compatibility section) — but it is *not*
proof the browser release itself is bcf-compatible, since that would require
driving the actual WasmTex pdflatex; see Limits below.

Three Biber sourcing options were evaluated, in the order docs/specs/wasmtex.md
requests, with the full evaluation and evidence in
[sources.json](sources.json):

1. **Debian bookworm's `biber` package** — rejected. `apt-cache policy biber`
   inside the i386 `bookworm-slim` container reports candidate `2.18-1`, too
   old.
2. **TeX Live's `i386-linux` binary from a frozen repository** — used.
   `https://texlive.info/historic/systems/texlive/2026/tlnet-final/` returns
   404 (2026 has no historic/final snapshot archived yet as of this build).
   The current rolling mirror
   (`https://mirror.ctan.org/systems/texlive/tlnet/`) does still ship an
   i386-linux `biber` binary, but its `texlive.tlpdb` carries no exact
   upstream version and is not pinned/reproducible. TeX Live **2025**'s
   frozen `tlnet-final` repository — the same one
   `latex/benchmark/candidates/tinytex-v86` already pins and validated —
   ships `biber.i386-linux` revision 75738, and running it reports exactly
   `biber version: 2.21`. Used this.
3. **CPAN into an i386 Perl** — not attempted; option 2 succeeded and is
   faster, smaller and already precedented in this repository.

## Build

```sh
node latex/tools/biber-vm/prepare.mjs   # fetches/verifies v86 runtime + biber binary
node latex/tools/biber-vm/build.mjs     # docker build, export, pack, write latex/mirror/biber-vm/<vmRelease>/
```

`build.mjs --export-only` reuses an already-built `komodoc-biber-vm:build`
Docker image and repeats only export/pack/registration — useful when only
v86-runtime files or `vm.json` metadata changed. The build is idempotent:
re-running it with unchanged inputs reproduces the same `<vmRelease>` and
copies zero new bytes.

Registration in `latex/mirror/manifest.json`
(`releases.<default_release>.vm = {id, url, sha256, size, biber}`) is
attempted first through `node latex/tools/wasmtex.mjs --vm <dir>` (package
A's documented interface). At the time of this build that flag exists but is
not implemented (it runs the ordinary mirror build and silently ignores
`--vm`), so `build.mjs` detects that the manifest's `vm` field was not
actually set and falls back to `latex/tools/biber-vm/register.mjs`, which
edits `manifest.json` additively — it touches only
`releases.<default_release>.vm` and nothing else. Once package A implements
`--vm`, `build.mjs` will use it automatically without any change here (it
verifies the manifest was actually updated, not just that the subprocess
exited 0).

## Test

```sh
node latex/tools/biber-vm/vm-smoke.mjs [vmRelease]
```

Boots the built release in headless Chromium via `web/tools/browser-driver.mjs`,
using the same mount / `KOMODOC_VM_READY` boot protocol as
`latex/benchmark/candidates/tinytex-v86/worker.js` (Buildroot kernel boots
first; the packed Debian+Biber rootfs is mounted over 9p at `/mnt` and
subsequent commands run via `chroot /mnt`). Runs `biber --version`, then a
real Biber job on the official biblatex sorting example (Unicode author
names, sorting, real citations) — reused from
`latex/benchmark/candidates/hybrid-validation/fixture.mjs`, the same fixture
`tinytex-v86` validated Biber output against. Falls back to a small
hand-written Unicode fixture (`assets/fixture/`) if that module or the
`.cache/projects` corpus is unavailable.

Asserts the `.bbl` is non-empty and contains the Unicode author/title text
byte-exact (`Ecclésiastique`, `Über das Wesen der Götter`). Records boot
time, cold and warm Biber time, and bytes fetched from the static server's
request counter. Writes [RESULTS.md](RESULTS.md).

## Sizes (this build)

| | |
| --- | --- |
| Docker image (guest content) | ~51 MB |
| Exported/stripped rootfs | ~109 MB (3105 files) |
| Packed VM release directory (`latex/mirror/biber-vm/<vmRelease>/`) | see RESULTS.md / build.mjs output at build time |
| v86 runtime files (libv86.js + v86.wasm + seabios + vgabios + bzimage) | ~11.9 MB, reused byte-for-byte from `latex/benchmark/candidates/tinytex-v86`'s pinned/verified assets |

Compare with `latex/benchmark/candidates/tinytex-v86`'s guest (TinyTeX +
Biber + ACM packages), whose exported filesystem is about 692 MB: carrying
only Biber and no TeX distribution keeps this guest roughly 6x smaller.

## Timings

See [RESULTS.md](RESULTS.md) for the measured boot time, cold/warm Biber
time, and bytes transferred from the most recent `vm-smoke.mjs` run. As with
`tinytex-v86`'s REPORT.md, these are v86-CPU-bound numbers from one desktop
Chromium session, not a promised product latency; docs/specs/wasmtex.md explicitly
expects this route to be slower than native execution and asks that it be
measured, not assumed fast.

## Licences

| Component | Licence | Evidence |
| --- | --- | --- |
| v86 runtime (`libv86.js`, `v86.wasm`, BIOS images, Buildroot kernel) | BSD-2-Clause | `libv86.js` file header; https://github.com/copy/v86/blob/master/LICENSE |
| Debian `bookworm-slim` base image | Various OSI licences per Debian package | No packages beyond the base rootfs are installed in the guest; nothing beyond Debian's own base-image licensing applies |
| Biber | Artistic License 2.0 | `https://raw.githubusercontent.com/plk/biber/dev/README.md`: "modify it under the terms of the Artistic License 2.0" |
| Perl | Not shipped | The Biber TeX Live binary bundles its own private Perl runtime; no system Perl interpreter is installed in the guest image |

Full source URLs and digests: [sources.json](sources.json). No credentials
appear anywhere in the recipe or the built image. The guest has no network
device and makes no runtime network access.

## What is not done

- **Not a real browser-release `.bcf`.** `vm-smoke.mjs` compiles the fixture
  with the developer machine's local TeX Live 2025 `xelatex`, not the actual
  WasmTex/browser engine. Both currently agree on bcf version `3.11`
  (recorded in RESULTS.md), but this smoke test does not itself prove
  browser-release compatibility — that requires an end-to-end test that
  drives WasmTex in the browser to produce the `.bcf`, which belongs to
  milestone 6's full acceptance matrix (package B2/B3), not this recipe.
- **No lazy/lifecycle integration.** This directory only builds and smoke-
  tests the image and registers it in the manifest. The browser VM client
  (`web/src/lib/latex/vm.js` / `vm-worker.js`, package B3), cancellation,
  idle teardown, persistent caching, and the routing/eligibility rules in
  docs/specs/wasmtex.md are separate work packages and are not implemented here.
- **No reproducibility guarantee for the Debian package set.** Like
  `tinytex-v86`, the base image digest is pinned but Debian's package
  archive is not frozen to a dated snapshot; here that barely matters since
  no `apt-get install` runs at all (the guest is exactly the base image plus
  one copied binary), but it is worth recording for completeness.
- **No range-based/chunked delivery tuning.** `vm.json`'s `files`/`objects`
  are ready for the static mirror to serve with digests and immutable
  caching, matching production per section 6's contract, but no
  range-request or priority-loading behavior was implemented or measured
  here; that is server/client integration work, not guest packaging.
- **Serving in production**: `crates/komodoc/src/latex.rs` and
  `latex/tools/serve.mjs` are package A's files and were not touched. This
  recipe only writes static files under `latex/mirror/biber-vm/` for them to
  serve like any other immutable digest-named mirror content.
