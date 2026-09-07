# TinyTeX-based browser emulation experiment

The prototype successfully runs ordinary Linux TeX and real Biber in a Chromium
worker, with no installed user app or compilation server. The tested multifile,
Biber, and ACM documents match the native reference in extracted text, page count,
and bibliography bytes. The current v86 setup is too slow for Komodoc's desired
live-preview experience.

## What was built

A 32-bit Debian userland contains a TinyTeX-based installation: the official
small-core package list at a pinned revision, installed from the frozen TeX
Live 2025 final repository, plus document dependencies. v86 runs the existing
Linux programs; none of pdfTeX, XeTeX, Perl, Biber, or latexmk was ported to WASM
for this experiment. A Buildroot kernel supplied by v86's example boots the
guest and mounts the lazy filesystem. A worker exchanges source/output bytes
with the page, and the static server delivers compressed resources on demand.

This is not an official TinyTeX archive: those Linux archives are 64-bit, while
v86 needs 32-bit executables. Docker prepares the environment and generates
native references during development. It is absent from browser compilation.
See [README.md](README.md) for exact setup and limitations.

## Initial validated run

[summary-initial.json](summary-initial.json) contains the immutable input
checksums, image/catalog identities, and measurements. All cases in that run
share one freshly booted Chromium VM in the recorded order.

| Document | First compile | Cached prose edit | One settled TeX pass | Output |
| --- | ---: | ---: | ---: | --- |
| Multifile paper, pdfLaTeX + BibTeX | 27.3 s | 12.1 s | 4.7 s | 4 pages; native text and bibliography match |
| Official Biber sorting example, XeLaTeX + Biber | 133.1 s | 32.9 s | 24.3 s | 1 page; native text and bibliography match |

The first-compile timings exclude the separately measured 2.55-second VM boot
and source staging. The second case reuses resources fetched by the first;
these are not independent cold-start measurements. Single-engine timings use
an unchanged, already-settled document and do not replace a complete compile.

The multifile case fetched 20.13 MB cumulatively through its first PDF,
including boot and setup. The Biber case then fetched another 45.97 MB during
its first compile. Both prose-edit phases fetched zero additional bytes. The
full served filesystem describes about 692 MB of files, illustrating that
static distribution size and per-document downloads are different quantities.
The configured guest RAM is 512 MiB; total/peak browser memory was not measured.

The bibliography example ran actual Biber 2.21. Its first and edited `.bbl`
files are byte-for-byte identical to the matching native reference, SHA-256
`59f25593c817d6dbfa14ad860ae6961124173f7d8053c7d9ffe83b982b4053e3`.
The PDF's normalized extracted text matches too, including accents such as
`Ecclésiastique` and `Über ... Götter`. No unresolved-reference or missing-glyph
warnings remain in the final TeX pass. Both documents emit readable gzip
SyncTeX with the expected header; navigation coordinates were not tested.

The Biber prose edit did not rerun Biber, yet took 32.9 seconds. Direct XeLaTeX
still took 24.3 seconds. Thus avoiding redundant bibliography processing or
moving latexmk's orchestration into JavaScript cannot by itself remove most
of that case's warm latency. The initial full bibliography build used four
XeLaTeX passes, Biber, and xdvipdfmx.

## ACM validation after completing the package set

[summary.json](summary.json) records a separate, fresh-browser ACM run after
adding the missing `everyshi`, `oberdiek`, and `upquote` packages to the build.
Its image/catalog identity differs from the initial run; the earlier two
successful cases were not rerun against this final image.

| Document | First compile | Cached prose edit | One settled TeX pass | Output |
| --- | ---: | ---: | ---: | --- |
| ACM conference sample, pdfLaTeX + BibTeX | 91.1 s | 32.4 s | 21.3 s | 6 pages; native text and bibliography match |

Boot took 2.76 seconds separately. Total compressed downloads through the first
PDF were 23.11 MB, including boot and setup; the prose edit fetched zero bytes.
The initial PDF's normalized extracted text and page count match native TeX.
Both first and edited bibliographies match the native `.bbl` byte-for-byte,
and the edited PDF contains the inserted prose. Both SyncTeX headers are valid.
No unresolved-reference or missing-glyph warnings remain in the final TeX log;
the upstream sample does produce ordinary BibTeX warnings about incomplete
bibliography fields.

## Interpretation

This validates the architecture's basic compatibility mechanism: a normal Linux
filesystem and programs avoid the custom missing-file and Unicode-transfer
defects seen in earlier adapters. It does not prove complete Overleaf
compatibility, a minimal image, or acceptable performance on other browsers.

The small emulator download is not the main obstacle in this run. v86 CPU
execution time remains substantial after all required files are cached. A
different emulator, a saved-state approach, and a direct-WASM TeX/emulated-Biber
hybrid have not been measured here. None should be assigned the timings of
this full-v86 prototype without testing it.

Production adapters, offered distributions, and the shared package mirror were
not changed. All implementation and generated artifacts are confined to this
candidate directory, apart from its named Docker image.
