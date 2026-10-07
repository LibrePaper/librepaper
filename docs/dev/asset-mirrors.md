# Browser asset mirrors

The server binary embeds pins, not renderer binaries or LaTeX files. Browsers
fetch all five pinned assets from the mirror in `[assets].mirror`; the default
is [`DEFAULT_ASSET_MIRROR`](../../crates/librepaper-base/src/config/mod.rs).
The official production config points to an OVH bucket in `bhs`.

- `assets.lock` pins four WASM modules and one LaTeX release. Each WASM pin
  names a GitHub repository, tag, and SHA-256. The LaTeX pin names a
  `wasm-latex` release tag and the SHA-256 of its `MANIFEST.json`.
- The published keys are `wasm/<sha256>/<module>.wasm` and
  `latex/<sha256>/<release files>`. LaTeX `release.json` uses format 2; its
  engine records and bundle index identify the files in that release.
- `node tools/assets/pins.mjs fetch` verifies pinned module and CodeMirror source bytes.
  It writes WASM to `web/wasm/` and the binding to ignored build output under
  `web/vendor/`.

## Release order

Update only the pins for assets that changed; an application release does not
require new renderer and LaTeX releases.
If `assets.lock` is unchanged, there is no mirror update to publish.

1. For each changed renderer, publish its tagged release in the source
   repository, then update that module's row. For example:

   ```sh
   node tools/assets/pins.mjs update wasm --repo wasm-markdown --tag vX.Y.Z
   ```

2. For a changed LaTeX distribution, build and tag the release in the
   `wasm-latex` repository, then create its mirror directory. From this repo,
   `tools/assets/mirror build` runs that repository's `make mirror` target.
   Pin the single release directory:

   ```sh
   node tools/assets/pins.mjs update latex
   ```

   Both commands default to `../wasm-latex/mirror`. For another directory,
   pass its path to `pins update latex` and set `MIRROR` for `mirror build`,
   `check`, and `publish`.
3. Fetch the pinned renderer modules and binding, then check the local LaTeX
   mirror:

   ```sh
   node tools/assets/pins.mjs fetch
   tools/assets/mirror check
   ```

   `update latex` requires exactly one 64-character release directory whose
   name matches the manifest digest.
4. Run `tools/assets/mirror publish --dry-run`. It validates both local
   mirrors, checks GitHub release visibility, and runs the Chromium tutorial
   smoke without loading credentials or changing the bucket. Resolve failures,
   then run `tools/assets/mirror publish` to upload.
5. Commit the reviewed `assets.lock` pin changes and build/deploy a server
   binary with those pins. Publish assets before deploying that binary.

## Publishing requirements

- The publisher validates files before upload, configures bucket CORS, probes
  public reads, uploads WASM then LaTeX, and checks object metadata and private
  bucket listing. Each LaTeX `release.json` is uploaded last.
- Objects are public-read, content-addressed, cached immutable, and append-only;
  the publisher never deletes them.
- Even a dry run needs network access, Rust, Bun, Node.js, Chromium, and
  PostgreSQL: it checks release URLs and builds/runs the tutorial smoke test.
- Set `LIBREPAPER_TEST_POSTGRES_URL` for an existing test database; otherwise
  Docker starts a temporary PostgreSQL container.
- Real uploads need AWS CLI v2 (or Nix) and credentials from the environment
  or SOPS.

- SOPS file: `tools/deploy/keys.yaml`. Keys: `MIRROR_S3_ENDPOINT`, `MIRROR_S3_REGION`,
  `MIRROR_S3_ACCESS_KEY_ID`, `MIRROR_S3_SECRET_ACCESS_KEY`, `MIRROR_S3_BUCKET`.
- Overrides: `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY`. Set `S3_PUBLIC_BASE_URL` for a custom endpoint or region.
- Use a dedicated OVH S3 user; let the publisher create the bucket under that
  identity so it has the ownership required to set CORS.

## Host a copy

Set the server config to a base HTTPS URL. The host must serve the pinned paths
and allow browser CORS `GET` and `HEAD` requests.

```toml
[assets]
mirror = "https://host/"
```

Check a hosted LaTeX release with `tools/assets/mirror check
https://host/latex/<sha256>/`.
