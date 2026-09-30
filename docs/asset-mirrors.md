# Browser asset mirrors

The asset mirror is one dedicated OVH S3 bucket that serves two things to
browsers: the four browser wasm modules (markdown, bibliography, citations and
typst) under `wasm/<sha256>/<module>`, and the LaTeX mirror under `latex/`.
The server embeds neither; it hands browsers the mirror's base URL
(`--asset-mirror`, default `DEFAULT_ASSET_MIRROR` in
`crates/librepaper/src/config.rs`) and they fetch from it.

The wasm modules are the ones pinned in `wasm-modules.lock`. Run `make wasm`
to fetch them into `web/wasm/`; the publisher stages them as
`<sha256>/<module>`, refuses any file whose bytes do not hash to the lock, and
publishes them with the `wasm` prefix. Build the LaTeX mirror first in the
sibling `wasm-latex` repository (`make wasm && make mirrors`). Install Node.js, SOPS and AWS CLI v2, and
keep the encrypted publisher keys in `deploy/keys.yaml`. `make mirrors-push`
uses `nix shell nixpkgs#awscli2` when AWS CLI v2 is missing and Nix is available.

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
wasm modules and the local LaTeX mirror are integrity-checked before either upload begins. The publisher reads
each uploaded object back and verifies its bytes and response metadata before
publishing release indexes. Each upload uses a public-read ACL (listing stays
private). `deploy/deploy-mirror.sh` creates the bucket as the publisher user,
sets CORS once, pushes both mirrors, and verifies public reads; `--test`
uploads two small files (`test/markdown.wasm` and `test/_headers`) instead of the mirrors. The application's default mirror URL remains manual.

Publisher credentials need object read/write access. Allow anonymous reads only
for `wasm/*` and `latex/*`; keep listing private. Uploads use gzip where suitable,
immutable caching for hashed assets, `no-store` for manifests, and `no-cache`
for bundle indexes. Existing objects are retained.

Keep the existing mirror URLs live until the OVH objects are reachable over
HTTPS and browser checks pass. Only then update `DEFAULT_ASSET_MIRROR`. The
current publishing setup does not perform that cutover.

A module is addressed by the SHA-256 of its own bytes, so a URL can never name
different bytes; moving a pin in `wasm-modules.lock` (`make wasm-update`) and
publishing again adds a new object beside the old one.
