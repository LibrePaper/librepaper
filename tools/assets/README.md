# Browser asset tools

The LaTeX mirror is built in the **wasm-latex** repository (`make mirror` there; layout in `wasm-latex/docs/release.md`) and published from here. A release is `latex/<id>/` where `<id>` is SHA-256 of `MANIFEST.json`, immutable and append-only, described by `release.json` (format 2, bundled releases only). The release a build uses is pinned in `assets.lock`.

## Commands

`tools/assets/pins`: fetch wasm modules and update assets.lock

`tools/assets/mirror build`: build LaTeX mirror (`make -C <MIRROR dir>`)

`tools/assets/mirror check <url|dir>`: validate LaTeX mirror (directory also gets wasm dry-run); no credentials, no network for a directory

`tools/assets/mirror smoke`: compile LaTeX tutorial in Chromium against MIRROR

`tools/assets/mirror publish`: check, smoke, probe bucket, upload everything; or `--dry-run`

Publishing the mirror happens only from this repository with `tools/assets/mirror publish`.

## LaTeX release format

A release directory `<id>/` must contain:
- `MANIFEST.json`: arbitrary JSON; id = SHA-256 of this file
- `release.json`: format 2, every path relative to the directory, bundled releases only (no per-file TeX snapshot)
- Engine files: each listed in a release.json engine worker inventory with size and sha256
- `bundles/bundles.json`: the LaTeX package index, must match sha256 pin in release.json; every bundle path in it must exist on disk
- Bundle tars: `bundles/b/<sha256>/<slug>.tar` where sha256 matches the tar bytes
