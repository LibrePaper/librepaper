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
  - `latex` row: `latex wasm-latex <tag> <sha256>`, set by `tools/pins update latex`
- `web/wasm/`: fetched modules, for tests and publishing only

## Pins

```sh
tools/pins fetch                             # fetch and verify the pinned modules
tools/pins update wasm wasm-markdown vX.Y.Z  # move a module pin
```

## Updating the LaTeX engines

In `../wasm-latex`:

```sh
make vendor                                # once: verified TeX Live tree (5 GB)
make rebuild                               # build engines and data (~2 h)
git add receipts/ && git commit -m "Record receipts"
make release                               # tags engines-YYYY.MM.DD (-2, -3 if taken); TAG= overrides
make mirror                                # prints the release hash
```

- `make release` refuses a dirty tree, an existing tag, or no `gh` login
- Tags never move; the release hash (sha256 of `staged/MANIFEST.json`) is its identity

Then here:

```sh
tools/pins update latex                    # pin the one release in ../wasm-latex/mirror
tools/deploy-assets publish                # check, smoke, probe, upload before committing
git commit assets.lock -m "Pin engines-YYYY.MM.DD"
```

## Publishing

```sh
tools/pins fetch && tools/deploy-assets build
tools/pins update latex
tools/deploy-assets publish --dry-run    # stages 1-3 only; no credentials, no bucket
tools/deploy-assets publish              # check, smoke, probe, upload
```

- Needs Node.js, SOPS, AWS CLI v2 (falls back to `nix shell nixpkgs#awscli2`)
- Keys: `tools/deploy-keys.yaml` (SOPS)
  - `OVH_S3_ENDPOINT`, `OVH_S3_REGION`, `OVH_S3_USER`, `OVH_S3_SECRET`
  - `OVH_S3_ARN` (`arn:aws:s3:::BUCKET`) names the bucket
  - overridden by `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
  - set `S3_PUBLIC_BASE_URL` to the public HTTPS bucket URL when overriding `S3_ENDPOINT` or `S3_REGION`; publish probes use it for object and private-listing checks
- One-time: create an S3 user in the OVH console, not the bucket; `publish` creates it so the user owns it (only the owner can set CORS)
- `publish` does, in order:
  1. check mirror: modules match their pins; LaTeX directory matches its manifest, nothing missing or extra
  2. verify GitHub releases are public (not drafts)
  3. smoke: compile the tutorial in Chromium
  4. create bucket if needed, set CORS (GET, HEAD, any origin)
  5. upload two small test files, verify public read with CORS
  6. upload all assets (`aws s3 sync --size-only --acl public-read`, gzip level 6, never `--delete`), each `release.json` last so no release is visible half uploaded; verify public read with CORS, verify bucket listing is private

## Properties

- Every key is content-addressed and cached immutable; nothing is rewritten
- Append-only: old pins keep working; no pruning
- Upload before shipping a binary with a new pin

## Hosting your own copy

```sh
librepaper admin serve --asset-mirror https://host/   # serve the pinned paths over HTTPS, CORS for GET and HEAD
tools/deploy-assets check https://host/latex/<sha256>/      # validate a LaTeX release
```
