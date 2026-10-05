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
- `tools/assets/pins fetch` verifies pinned module and CodeMirror source bytes.
  It writes WASM to `web/wasm/` and the binding to ignored build output under
  `web/vendor/`.

## Release order

Update only the pins for assets that changed; an application release does not
require new renderer and LaTeX releases.
If `assets.lock` is unchanged, there is no mirror update to publish.

1. For each changed renderer, publish its tagged release in the source
   repository, then update that module's row. For example:

   ```sh
   tools/assets/pins update wasm wasm-markdown vX.Y.Z
   ```

2. For a changed LaTeX distribution, build and tag the release in the
   `wasm-latex` repository, then create its mirror directory. From this repo,
   `tools/assets/mirror build` runs that repository's `make mirror` target.
   Pin the single release directory:

   ```sh
   tools/assets/pins update latex
   ```

   Both commands default to `../wasm-latex/mirror`. For another directory,
   pass its path to `pins update latex` and set `MIRROR` for `mirror build`,
   `check`, and `publish`.
3. Fetch the pinned renderer modules and binding, then check the local LaTeX
   mirror:

   ```sh
   tools/assets/pins fetch
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

The publisher checks the pinned files and LaTeX release contents before any
upload. It uploads WASM, then LaTeX; each LaTeX `release.json` is uploaded last
so clients cannot discover a partial release. It creates the bucket if needed,
sets CORS for `GET` and `HEAD`, probes public reads, checks uploaded object
metadata, and confirms bucket listing is private. Uploads are public-read,
content-addressed, immutable, and append-only; the publisher never deletes
objects.

The dry run also fetches/checks public release URLs and builds the server and
browser smoke, so it needs network access, Rust, Bun, Node.js, Chromium, and
PostgreSQL. Set `LIBREPAPER_TEST_POSTGRES_URL` to use an existing test database;
otherwise Docker starts a temporary PostgreSQL container. A real upload also
needs SOPS and AWS CLI v2 (or Nix for the temporary CLI shell).

SOPS keys come from `tools/deploy/keys.yaml`: `OVH_S3_ENDPOINT`,
`OVH_S3_REGION`, `OVH_S3_USER`, `OVH_S3_SECRET`, and `OVH_S3_ARN`. Canonical
overrides are `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`,
`AWS_ACCESS_KEY_ID`, and `AWS_SECRET_ACCESS_KEY`. Set `S3_PUBLIC_BASE_URL` when
overriding the endpoint or region.

Use a dedicated OVH S3 user. The publisher creates the bucket with that
identity and then sets CORS, which requires bucket-owner permissions.

## Host a copy

Set the server config to a base HTTPS URL. The host must serve the pinned paths
and allow browser CORS `GET` and `HEAD` requests.

```toml
[assets]
mirror = "https://host/"
```

Check a hosted LaTeX release with `tools/assets/mirror check
https://host/latex/<sha256>/`.
