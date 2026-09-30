# Browser asset mirrors

Browsers load their renderers and LaTeX files from one S3 bucket on OVH (region `bhs`). It
serves two things: the four browser wasm modules (markdown, bibliography,
citations and typst) under `wasm/<sha256>/<name>.wasm`, and the LaTeX engines
and TeX Live bundles under `latex/<sha256>/`. The binary embeds neither. The
server hands browsers the mirror's base URL (`--asset-mirror`, default
`DEFAULT_ASSET_MIRROR` in `crates/librepaper/src/config.rs`) and the pinned
LaTeX release directory (`latexMirror` in `/api/config`), and they fetch from
those.

`assets.lock` pins all five and is compiled into the binary. Each `*.wasm` row
pins a module (repository, tag, sha256 of the module); the `latex` row
(`latex wasm-latex <tag> <sha256>`) pins the release directory `latex/<sha256>/`
by the sha256 of its `MANIFEST.json`. A browser never chooses a release; it uses
the one its server was built with.

`make wasm` fetches the pinned modules from public GitHub releases into
`web/wasm/`. The binary does not need them; tests, tools and publishing do.
`make wasm-update REPO=wasm-markdown TAG=vX.Y.Z` moves a module pin. The `latex`
row is edited by hand.

A LaTeX release is an immutable directory, `latex/<id>/`, where `<id>` is the
SHA-256 of `<id>/MANIFEST.json`. It holds `release.json` (format 2, every path
relative to that directory), `MANIFEST.json`, the engine files, and `bundles/`
with its index `bundles/bundles.json`. There is no top-level manifest and no
default release. The LaTeX mirror is built in the sibling `wasm-latex`
repository with `make mirror`, which holds exactly one release; `make mirrors`
runs that from here. `MIRROR=` points at its output (default
`../wasm-latex/mirror`).

## Publishing

Install Node.js, SOPS and AWS CLI v2 (`make mirrors-push` falls back to
`nix shell nixpkgs#awscli2` when AWS is missing and Nix is present), and keep
the encrypted publisher keys in `deploy/keys.yaml`. The file supplies
`OVH_S3_ENDPOINT`, `OVH_S3_REGION`, `OVH_S3_USER` (the S3 access key),
`OVH_S3_SECRET` and `OVH_S3_ARN` (`arn:aws:s3:::BUCKET`, which names the
bucket). `OVH_S3_HOST` is unused. `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`,
`AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` in the environment override the
mapped values.

One-time setup: under Public Cloud, Object Storage, Users, create an S3 user and
generate its S3 credentials. Do not create the bucket in the console.
`deploy/deploy-mirror.sh` creates it with that key so the user owns it, because
OVH lets only the owner set CORS.

```sh
make wasm && make mirrors

# Validate the modules and the LaTeX mirror; no credentials or network needed.
make mirrors-push MIRRORS_DRY_RUN=1

# Check the bucket, CORS and connectivity by uploading two small test files.
deploy/deploy-mirror.sh --test

# Publish.
deploy/deploy-mirror.sh
```

`deploy/deploy-mirror.sh` reads the keys through SOPS. It first checks that
every GitHub release `assets.lock` pins is public (a draft answers 404 to
anyone signed out). It then creates the bucket if needed, sets CORS (GET and
HEAD from any origin), runs `make mirrors-push`, and verifies that a pinned
object is publicly readable with a CORS header and that listing the bucket is
not public.

`make mirrors-push` runs `tools/push-mirrors.mjs` and `tools/publish-mirror.mjs`.
A preflight checks that each module hashes to its pin and that the LaTeX
directory is `<sha256>/` with a `MANIFEST.json` that hashes to the directory
name, a format 2 `release.json`, every file and bundle it names present with
the recorded size and digest, and nothing unnamed. The publisher then stages
the files in a temporary directory (gzip level 6, with progress on stderr),
groups them by content type, encoding and cache control, and uploads each group
with `aws s3 sync --size-only --acl public-read` at 32 concurrent requests.
Sync skips keys that already exist with the same size, which is safe because
every key is content-addressed. It never passes `--delete`. Each release's
`release.json` goes last, so a release is never visible half uploaded. A
`head-object` spot check of a few keys confirms size and headers.

## Properties

Every object is named by a digest or lives under one, so nothing is ever
rewritten. Everything is cached immutable, and there is no mutable file. The
bucket is append-only: a new pin adds an object or directory beside the old
ones, and pruning is not implemented. Upload before shipping a binary that
carries a new pin; older binaries keep working because nothing is deleted.
Each object is uploaded with a public-read ACL, so anonymous reads work per
object while listing the bucket stays private.

## Hosting your own copy

An operator may host a copy of the tree for the pins of their binary and pass
`--asset-mirror https://host/` (or `LIBREPAPER_ASSET_MIRROR`). The host must
serve those exact paths over HTTPS with CORS for GET and HEAD.
`make latex-check MIRROR=<dir or url>` validates a LaTeX release before a
deployment points at it.
