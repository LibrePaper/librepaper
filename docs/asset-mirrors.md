# Browser asset mirrors

The asset mirror is one dedicated OVH S3 bucket that serves two things to
browsers: the four browser wasm modules (markdown, bibliography, citations and
typst) under `wasm/<sha256>/<module>`, and the LaTeX engines and TeX Live bundles
under `latex/<sha256>/`. The server embeds neither; it hands browsers the
mirror's base URL (`--asset-mirror`, default `DEFAULT_ASSET_MIRROR` in
`crates/librepaper/src/config.rs`) and the pinned LaTeX release directory
(`latexMirror` in `/api/config`), and they fetch from those.

`assets.lock` pins everything the mirror serves. Its four `*.wasm` rows pin the
wasm modules by digest; its `latex` row (`latex wasm-latex <tag> <sha256>`)
pins the LaTeX release directory `latex/<sha256>/`. The browser never chooses
a release; it uses the one its server was built with. Run `make wasm`
to fetch them into `web/wasm/`; the publisher stages them as
`<sha256>/<module>`, refuses any file whose bytes do not hash to the lock, and
publishes them with the `wasm` prefix. Build the LaTeX mirror first in the
sibling `wasm-latex` repository (`make wasm && make mirrors`). Install Node.js, SOPS and AWS CLI v2, and
keep the encrypted publisher keys in `deploy/keys.yaml`. `make mirrors-push`
uses `nix shell nixpkgs#awscli2` when AWS CLI v2 is missing and Nix is available.

A LaTeX release is an immutable directory, `latex/<id>/`, where `<id>` is the
SHA-256 of `<id>/MANIFEST.json`. `<id>/release.json` (format 2) describes it,
with every path relative to that directory: the engine files, the bundles index
(`bundles/bundles.json`) and the bundles beneath it. There is no top-level
manifest and no default release. The publisher refuses a mirror directory
unless each top-level directory is `<sha256>/`, its `MANIFEST.json` hashes to
the directory name, `release.json` is format 2, every file and bundle it names
is present with the size and digest it records, and nothing in the directory
is unnamed.

The key file supplies `OVH_S3_ENDPOINT`, `OVH_S3_REGION` (currently `bhs`),
`OVH_S3_USER`, `OVH_S3_SECRET`, and `OVH_S3_ARN`. `OVH_S3_USER` is the S3
access key ID. The exact bucket ARN may supply the bucket name; otherwise set
`S3_BUCKET` in the environment or `OVH_S3_BUCKET` in the key file. Only an ARN
of the form `arn:aws:s3:::BUCKET` is accepted. `OVH_S3_HOST` is unused. Canonical environment variables
`S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `AWS_ACCESS_KEY_ID`, and
`AWS_SECRET_ACCESS_KEY` override the corresponding mapped values.

```sh
# Check the wasm modules and the LaTeX mirror and their hashes; no credentials or network used.
make mirrors-push MIRRORS_DRY_RUN=1

# Publish both. KEYS and MIRROR (the LaTeX mirror directory) can be overridden.
make mirrors-push
```

SOPS decrypts the key file into memory for the publishing process. The staged
wasm modules and the local LaTeX mirror are integrity-checked before either upload begins.
The publisher stages the mirror in a temporary directory (gzipping what the
metadata says to gzip), groups files by content type, encoding and cache
control, and uploads each group with one `aws s3 sync` at 32 concurrent
requests. Sync skips keys that already exist with the same size, which is safe
because every key is content-addressed, and it never passes `--delete`. There is
no per-object readback: integrity rests on the CLI's upload checksums, plus a
`head-object` spot check of a few keys (the largest file and a `release.json`)
for size and headers. Each release's `release.json` is synced last, in a final
pass, so a release is never visible half uploaded. Each upload uses a public-read
ACL (listing stays private). Progress goes to stderr, one line per group. `deploy/deploy-mirror.sh` creates the bucket as the publisher user,
sets CORS once, pushes both mirrors, and verifies public reads; `--test`
uploads two small files (`test/markdown.wasm` and `test/_headers`) instead of the mirrors. The application's default mirror URL remains manual.

Publisher credentials need object read/write access. Allow anonymous reads only
for `wasm/*` and `latex/*`; keep listing private. Uploads use gzip where suitable,
immutable caching for everything: every object is named by a digest or lives
under one, so nothing on the mirror is ever rewritten and there are no
no-store or no-cache special cases. The mirror is append-only: publishing a
new release adds a directory beside the old ones, and existing objects are
retained, so binaries built against an older pin keep working.

Keep the existing mirror URLs live until the OVH objects are reachable over
HTTPS and browser checks pass. Only then update `DEFAULT_ASSET_MIRROR`. The
current publishing setup does not perform that cutover.

A module is addressed by the SHA-256 of its own bytes, and a LaTeX release by the
SHA-256 of its `MANIFEST.json`, so a URL can never name different bytes; moving
a pin in `assets.lock` (`make wasm-update` for a wasm module) and publishing
again adds a new object or directory beside the old one.
