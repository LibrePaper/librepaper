# LaTeX mirror tools

LibrePaper consumes a deployed WasmTex mirror -- engines and the TeX Live
package bundles -- built and pushed from the **wasm-latex** repository
(`make mirror`, `make push` there; layout and manifest documented in
`wasm-latex/docs/mirror.md`, format 1, bundled releases only, no per-file
TeX Live snapshot, no bloom filter). Nothing in this repository builds that
mirror any more. What is still here:

## Consumer-side check

`latex/tools/check-mirror.mjs` rejects a mirror before a deployment uses it:

```sh
node latex/tools/check-mirror.mjs latex/mirror                       # a local directory
node latex/tools/check-mirror.mjs https://librepaper-latex.<account>.workers.dev/
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
WasmTex mirror:

```sh
node latex/tools/biber-vm/build.mjs               # builds the VM (needs Docker)
node latex/tools/wasmtex.mjs --vm latex/mirror/biber-vm/<vmRelease>   # registers it
```

`build.mjs` calls `wasmtex.mjs --vm` itself once the VM is built, falling
back to `latex/tools/biber-vm/register.mjs` if that flag is not yet
implemented. Registration writes `releases.<default_release>.vm` into
`manifest.json`, per `docs/specs/wasmtex-interfaces.md` section 6 -- so the
mirror's default release must already exist (imported by wasm-latex's
tooling) before this can run.

## Reproduction status

Build reproduction and source receipts belong to the wasm-latex repository.
The mirror retains those receipts and the source URL from its staged
release; the manifest leaves `source.reproduced` false where wasm-latex has
not independently rebuilt an engine from source yet.
