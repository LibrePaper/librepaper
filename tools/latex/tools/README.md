# LaTeX mirror tools

LibrePaper consumes a deployed LaTeX engine mirror -- engines and the TeX Live
package bundles -- built and pushed from the **wasm-latex** repository
(`make mirror`, `make push` there; layout documented in
`wasm-latex/docs/mirror.md`). Nothing in this repository builds that mirror any
more. A release is an immutable directory `latex/<id>/`, where `<id>` is the
SHA-256 of `<id>/MANIFEST.json`, described by `<id>/release.json` (format 2,
every path relative to the release directory, bundled releases only). The
mirror is append-only; the release a build uses is pinned in `assets.lock`.
What is still here:

## Consumer-side check

`tools/latex/tools/check-mirror.mjs` rejects a release before a deployment uses it:

```sh
node tools/latex/tools/check-mirror.mjs tools/latex/mirror                     # every <id>/ in a mirror directory
node tools/latex/tools/check-mirror.mjs tools/latex/mirror/<id>                # one release directory
node tools/latex/tools/check-mirror.mjs https://assets.example/latex/<id>/     # a release URL
```

It checks that `release.json` parses, `format === 2`, the release has a
complete pdfTeX engine, and `bundles` is present. Given a directory it also
verifies that the directory name is the SHA-256 of `MANIFEST.json` and checks
every engine file and bundle tar on disk against `release.json` and
`bundles.json`'s digests. `deploy/assets check <url or dir>` runs it;
`MIRROR=<dir> deploy/assets smoke` (default `../wasm-latex/mirror`, where
wasm-latex builds it) runs it and then compiles and displays the seeded LaTeX
example in headless Chromium.

## Reproduction status

Build reproduction and source receipts belong to the wasm-latex repository.
The mirror retains those receipts and the source URL from its staged
release; `release.json` leaves `source.reproduced` false where wasm-latex has
not independently rebuilt an engine from source yet.
