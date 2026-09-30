# Browser asset mirrors

Browsers fetch renderers and LaTeX files from one OVH S3 bucket (`bhs`). The binary embeds neither.

- `wasm/<sha256>/<name>.wasm`: markdown, bibliography, citations, typst
- `latex/<sha256>/`: one LaTeX release; `<sha256>` hashes its `MANIFEST.json`
  - `release.json` (format 2, paths relative to the directory)
  - `MANIFEST.json`
  - engine files
  - `bundles/` and its index `bundles/bundles.json`
- Base URL: `--asset-mirror` or `LIBREPAPER_ASSET_MIRROR`, default `DEFAULT_ASSET_MIRROR` in `crates/librepaper/src/config.rs`
- `assets.lock`: pins all five, compiled into the binary; browsers use the server's pin (`latexMirror` in `/api/config`)
  - `*.wasm` rows: repository, tag, sha256 of the module
  - `latex` row: `latex wasm-latex <tag> <sha256>`, edited by hand
- `web/wasm/`: fetched modules, for tests and publishing only

## Pins

```sh
tools/pins fetch                             # fetch and verify the pinned modules
tools/pins update wasm wasm-markdown vX.Y.Z  # move a module pin
```

## Updating the LaTeX engines

```sh
cd ../wasm-latex                   # README: make rebuild, make release, make mirror
cd ../librepaper
deploy/assets check                # validate ../wasm-latex/mirror
deploy/assets smoke                # compile the tutorial in Chromium against it
deploy/assets publish --test
deploy/assets publish
# then: assets.lock latex row -> new tag and hash
```

## Publishing

```sh
tools/pins fetch && deploy/assets build
deploy/assets publish --dry-run    # no credentials, no network
deploy/assets publish --test       # two small files: bucket, CORS, connectivity
deploy/assets publish
```

- Needs Node.js, SOPS, AWS CLI v2 (falls back to `nix shell nixpkgs#awscli2`)
- Keys: `deploy/keys.yaml` (SOPS)
  - `OVH_S3_ENDPOINT`, `OVH_S3_REGION`, `OVH_S3_USER`, `OVH_S3_SECRET`
  - `OVH_S3_ARN` (`arn:aws:s3:::BUCKET`) names the bucket
  - overridden by `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
- One-time: create an S3 user in the OVH console, not the bucket; `publish` creates it so the user owns it (only the owner can set CORS)
- `publish` does, in order:
  - refuses if any pinned GitHub release is a draft
  - creates the bucket if needed, sets CORS (GET, HEAD, any origin)
  - preflight: modules match their pins; the LaTeX directory matches its manifest, nothing missing or extra
  - `aws s3 sync --size-only --acl public-read`, gzip level 6, never `--delete`
  - each `release.json` last, so no release is visible half uploaded
  - spot checks: public read with CORS works, bucket listing does not

## Properties

- Every key is content-addressed and cached immutable; nothing is rewritten
- Append-only: old pins keep working; no pruning
- Upload before shipping a binary with a new pin

## Hosting your own copy

```sh
librepaper admin serve --asset-mirror https://host/   # serve the pinned paths over HTTPS, CORS for GET and HEAD
deploy/assets check https://host/latex/<sha256>/      # validate a LaTeX release
```
