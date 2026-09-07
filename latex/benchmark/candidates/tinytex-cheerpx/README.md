# TinyTeX / CheerpX benchmark

Isolated technical evaluation of CheerpX 1.2.8 running the existing 32-bit
TinyTeX-based TeX Live 2025 userland. Production adapters are unaffected.

The runtime is imported from
`https://cxrtnc.leaningtech.com/1.2.8/cx.esm.js`, with attribution in the page.
No CheerpX runtime is vendored or served locally. Its community licence
requires the hosted runtime; self-hosting requires a separate licence.
The benchmark's own ext2 disk is served locally via HTTP byte ranges.

## Reproduce

Requires the prepared `../tinytex-v86/assets/rootfs` and its matching native
reference results, plus Node, Chromium, Poppler and `mke2fs` on the developer's
machine. End users would need only their browser.

```sh
node latex/benchmark/candidates/tinytex-cheerpx/build.mjs
node latex/benchmark/candidates/tinytex-cheerpx/run.mjs
```

The build packages a 1100 MiB ext2 filesystem without mounting it, records its
SHA-256 and source image receipt, verifies the native reference input bytes,
and copies the reference PDFs and bibliographies into this candidate.
The source candidate is read only. The current adapter uses guest UID 1000,
GID 100, matching ownership of this host's extracted source filesystem.
Other ownership requires adjusting those values or normalizing the image.

The server binds to 127.0.0.1:8705, Chromium DevTools to 9705, and a fresh
candidate-specific profile is created for each run. The existing profile and
selected output paths are replaced on rerun. `inspect.mjs` reads only this
candidate's active browser. No compilation service runs on the host.

## Measurement boundaries

One Chromium environment is reused across the two documents. First builds
are measured in sequence, so the second shares cached TeX resources with the
first. Prose edits preserve auxiliary files. The separate settled-engine pass
is not an edit or a complete reference-convergence pipeline. The bibliography
edit changes the title of a cited book and must both invoke Biber and show the
new title in the PDF.

Compile timings include command dispatch and up to 150 ms of completion
polling, but exclude source staging and PDF export. Guest command timings are
also recorded. There is no deliberate CPU or network throttling. Native
reference files are reused after source-byte verification, not recompiled as
part of these timings.

Local HTTP counters measure uncompressed disk-range response bodies. They
exclude HTTP headers and the externally hosted CheerpX runtime. They are not
directly comparable to v86's gzip-compressed file downloads. Runtime Resource
Timing entries are recorded where exposed; cross-origin restrictions may
hide byte sizes and worker requests. The disk size is not its download size.
Total/peak memory, mobile/Safari, persistent-cache reloads, and visual PDF
equivalence are not measured.

## Compatibility adaptations

- `DataDevice` inputs use unique names. Replacing an existing entry failed.
- Input staging uses `cat`; `cp` stalled for over 120 seconds in the initial
  attempt. This has not been reduced to an upstream bug report.
- Projects compile on the ext2 overlay. Output files are copied to a separate
  IDB directory mount for byte extraction after timing completes.
- Overwritten diagnostic logs retained old timestamps, confusing latexmk on
  both mount types tested. The engine command removes only its `.log` before
  each pass, leaving auxiliary and bibliography files in place. Timings include
  this workaround. This does not establish general timestamp correctness.
- Guest ownership matches the extracted image so Biber's packed Perl runtime
  can safely create its extraction directory. Biber itself is unmodified.

## Biber-only bridge contract

The browser adapter exposes the smallest useful bridge for a hybrid runtime:
stage the document's `.bcf` and every referenced `.bib` as byte arrays in one
temporary guest directory, then run Biber with that directory as its working
directory. The equivalent operation is:

```text
biber --output-format=bbl <stem>.bcf
```

Return the resulting `<stem>.bbl` and `<stem>.blg` byte arrays, the exit code,
and elapsed milliseconds measured around the guest command. Keep the `.bcf`
and `.bib` bytes unchanged; Biber resolves bibliography paths recorded in the
`.bcf`. A bridge can use the existing `vmSend({type: 'write', ...})`,
`vmSend({type: 'command', ...})`, and `vmSend({type: 'read', ...})` messages in
`adapter.js`, with shell-quoted paths and a fresh per-request directory. This
keeps Biber out of the TeX pipeline while preserving its native output bytes.

The implemented standalone `biber-bridge.js` now creates its own lazy CheerpX
instance and does not depend on the adapter globals. It supports a root-level
BCF basename and relative bibliography/configuration paths, serializes
requests, and returns raw BBL/BLG bytes plus separate initialization, staging,
command, and export timings. Guest-root access is used only to copy output
into the initially root-owned IDB export mount; Biber runs as UID 1000.

Run `node latex/benchmark/candidates/tinytex-cheerpx/smoke.mjs` after generating
the sibling hybrid-validation native references. The fresh-browser smoke
compares the BBL's SHA-256 with the native reference. Its successful result is
also exercised by the complete [hybrid experiment](../hybrid/REPORT.md).

Sources: [CheerpX custom images](https://cheerpx.io/docs/guides/custom-images),
[HTTP disk API](https://cheerpx.io/docs/reference/CheerpX.HttpBytesDevice/create),
[licensing](https://cheerpx.io/docs/licensing).
