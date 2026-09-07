# TinyTeX-based Linux guest in v86

This isolated prototype runs conventional 32-bit Linux TeX Live programs in a
v86 browser worker. Docker is used only to prepare the static guest filesystem
and generate matching native reference PDFs. Browser compilation has no Docker,
local application, or compilation-server dependency.
Measured results and interpretation are in [REPORT.md](REPORT.md).

## What “based on TinyTeX” means

TinyTeX's published Linux archives are 64-bit. v86 does not emulate x86-64, so
this is an adapted build, not an official TinyTeX binary release:

- TinyTeX's infraonly installation approach and `pkgs-custom.txt` at revision
  `d01200eb642187eb3b00b02084500844906cbeef` supply the small core.
- All TeX packages and binaries come from the frozen TeX Live 2025 final
  repository. The requested `i386-linux` programs run in Debian Bookworm's
  32-bit userland using the Linux kernel distributed with v86's examples.
- Biber/biblatex and the dependencies needed for the public ACM and bibliography
  fixtures are installed during the build. There is no runtime package
  installation or guest network connection. Missing files remain errors.
- The installer also detects the build host's x86-64 platform and includes
  unused binaries for it. They remain in this exploratory filesystem but the
  runtime PATH explicitly selects i386, and unused files are not fetched.

Sources: [TinyTeX recipe](https://github.com/rstudio/tinytex/tree/d01200eb642187eb3b00b02084500844906cbeef/tools),
[frozen TeX repository](https://texlive.info/historic/systems/texlive/2025/tlnet-final/),
[v86 filesystem format](https://github.com/copy/v86/blob/master/docs/filesystem.md),
[v86 serial example](https://github.com/copy/v86/blob/master/examples/serial.html).

## Reproduce

From the Komodoc repository root, with Node, Docker, curl, tar, Chromium, and
Poppler's `pdfinfo`/`pdftotext` available to the developer:

```sh
node latex/benchmark/prepare.mjs
node latex/benchmark/candidates/tinytex-v86/prepare.mjs
node latex/benchmark/candidates/tinytex-v86/build.mjs
node latex/benchmark/candidates/tinytex-v86/run.mjs multifile biber-sorting acm-conference
node latex/benchmark/candidates/tinytex-v86/summarize.mjs
```

`prepare.mjs` checks downloaded artifacts against `assets-lock.json`. Several
upstream asset URLs are mutable: preserving/mirroring the verified files is
necessary if those URLs later change. A digest mismatch stops preparation.

The Docker base image and TeX repository are pinned, but Debian apt updates
are not frozen to a dated snapshot. Thus the recipe is repeatable but is not
yet a bit-for-bit reproducible release build. Image/filesystem receipts are
written under `assets/`. Package provenance is retained in the installed TeX
database. Upstream release notices remain in the guest filesystem.

`build.mjs --export-only` repackages an already built image. `run.mjs --boot-only`
boots the guest and runs the version probes. The HTTP server binds only to
127.0.0.1:8704; Chromium DevTools uses 9704. Raw outputs, profiles, filesystem
objects, and the exported tar are ignored by Git.
Each run replaces `results/report.json` and the selected cases' output folders;
`summarize.mjs` explicitly replaces `summary.json`. The preserved
`summary-initial.json` identifies the earlier two successful cases before the
remaining ACM package dependencies were added.

## Measurement boundaries

The run starts with a fresh Chromium profile and one 512 MiB guest. All cases
share that VM/cache in the specified order. “First” means the first compile of
that case, not an independently cold browser. Boot and per-phase network data
are recorded separately; lifetime counters include setup requests as well.
Version probes run after documents to avoid preloading Biber into a pdfLaTeX-only
startup. Network bytes are gzip-compressed HTTP response bodies served locally,
excluding headers. There is no network/CPU throttling.

The native reference uses the same Docker image, engine, source tree, and
compile command with networking disabled. The browser uses a 9p mount of its
exported filesystem. User inputs are transferred as bytes. The editor-facing
worker API is only an experimental harness, not a production API.

“Edit” changes visible prose and runs latexmk again while keeping the guest
alive. `engineOnly` times one direct, already-settled TeX pass; it is not a
complete edit or bibliography update. Diagnostics are checked in the final
TeX log, since undefined citations in earlier passes are normal. PDF page/text
comparison does not establish visual layout or bibliography-order equivalence.

The configured 512 MiB guest RAM is not a measurement of total browser memory.
Persistent cache across reload, mobile/Safari behavior, saved-machine snapshots,
and a tuned minimal guest kernel remain untested. The full static filesystem
size is not the amount downloaded by a document.
