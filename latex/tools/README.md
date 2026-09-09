# LaTeX mirror tools

LibrePaper consumes a deployed LaTeX engine mirror -- engines and the TeX Live
package bundles -- built and pushed from the **wasm-latex** repository
(`make mirror`, `make push` there; layout and manifest documented in
`wasm-latex/docs/mirror.md`, format 1, bundled releases only). Nothing in this
repository builds that mirror any more. What is still here:

## Consumer-side check

`latex/tools/check-mirror.mjs` rejects a mirror before a deployment uses it:

```sh
node latex/tools/check-mirror.mjs latex/mirror                       # a local directory
node latex/tools/check-mirror.mjs https://latex.librepaper.workers.dev/
```

It checks that `manifest.json` parses, `manifest.format === 1`, the default
release has a complete pdfTeX engine, and `bundles` is present on it. Given a
directory (not a URL) it also verifies every engine file and bundle tar on
disk against `bundles.json`'s digests. `make latex-check LATEX=<url or dir>`
runs it; `make latex-smoke MIRROR=<dir>` (default `../wasm-latex/mirror`,
where wasm-latex builds it) runs it and then compiles and displays the
seeded LaTeX example in headless Chromium.

## The Biber VM

LibrePaper still builds and deploys its own Biber VM, separately from the
engine mirror:

```sh
node latex/tools/biber-vm/build.mjs               # builds the VM (needs Docker)
node latex/tools/biber-vm/register.mjs <release-dir> <published-vm.json-url>
```

`register.mjs` only hashes the published descriptor and prints the
`--biber-vm <url>#<sha256>` flag. It never modifies the engine mirror.

## Reproduction status

Build reproduction and source receipts belong to the wasm-latex repository.
The mirror retains those receipts and the source URL from its staged
release; the manifest leaves `source.reproduced` false where wasm-latex has
not independently rebuilt an engine from source yet.
