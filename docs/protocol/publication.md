# Rendered publication API

Editors render a captured source tree locally and explicitly publish a display
bundle. Source saves, checkpoints, previews and reconnects never upload one.
Readers load the current bundle and use the annotation channel without source
synchronization. Before the first publication, metadata returns null.

All API calls carry `X-LibrePaper-Client`; link sessions also carry
`X-LibrePaper-Key`. The server resolves current authority on every request.

`GET /api/documents/:slug/publication` returns `{publication: null}` or a record
with `id`, `published_at`, `publisher`, and an absolute isolated-origin `html_url`.
Only editors receive `source_sha256` and `render_config_sha256`.

Publishing uses an `Idempotency-Key` and these operations:

1. `POST /api/documents/:slug/publication/prepare` with
   `{manifest, expected_publication_id}` returns `{missing: [sha256, ...]}`.
2. `PUT /api/documents/:slug/publication/objects/:sha256` uploads a missing
   object. `Content-Type` must match the descriptor. `Content-Encoding: gzip`
   is supported; limits and hashes are checked against the decoded bytes.
3. `POST /api/documents/:slug/publication/activate` sends the same prepare body
   and idempotency key, and returns `{publication: ...}` after activation.

The manifest has `bundle_sha256`, `source_sha256`, `render_config_sha256`,
`html: {sha256, bytes, mime}`, and
`assets: [{path, sha256, bytes, mime}]`. The bundle digest hashes UTF-8 JSON with
fields in this order: `{html: html.sha256, assets: [...]}`. Sort assets by UTF-8
path bytes and serialize their fields as `path`, `sha256`, `bytes`, `mime`.
The server supplies publication identity, timestamp and attribution.

A conflicting expected publication returns 409. A changed descriptor cannot
reuse a prepared key. Successful retries reuse the original attribution and
timestamp, including after completed staging objects have been removed.

Limits are 16 MiB decoded HTML, 64 MiB per asset, 512 assets, 512 UTF-8 bytes per
relative path, a 256 KiB manifest, and 256 MiB of staging per document. Two
unfinished preparations may coexist; they expire after 15 minutes. All stored
objects, staging and metadata count against owner and deployment storage limits.
Superseded objects are reclaimed after a 15-minute grace period once they are
unreferenced by the current bundle or an unfinished preparation.
Small retirement markers remain accounted but may use bounded cleanup headroom
above a full quota, so reaching the quota cannot prevent reclamation. Startup
and periodic reconciliation settle interrupted publication reservations and
release charges only after confirming that deleted objects are absent.
Publication delivery and object uploads share the deployment's artifact-transfer
concurrency limit; gzip uploads reserve decoded capacity before reading bodies.

Published HTML and its manifest assets are served only on the document origin.
A signed display capability conveys no source credential. Its live link or
account authority is rechecked for every response, including conditional asset
requests. HTML transfer supports gzip. Assets use stable document-scoped paths
and private revalidation so unchanged assets can reuse browser cache bytes.
Downloaded display bytes cannot be revoked retroactively.

The cost regression fixture uses a 1 MiB incompressible image and a small text
edit. It measures publication metadata separately from object bodies:

| Explicit publication | Object upload | Image upload | Current object storage | Reader object download | Cached objects reused |
| --- | ---: | ---: | ---: | ---: | ---: |
| First | 1,048,693 B | 1,048,576 B | 1,048,693 B | 1,048,693 B | 0 |
| Small text edit | 116 B | 0 B | 1,048,692 B | 116 B | 1 |
| New source identity, identical display | 0 B | 0 B | 1,048,692 B | 0 B | 2 |

Each row uses two publication metadata requests plus one explicit reader
metadata refresh. Prepare/activate request bodies total 1,252–1,260 bytes.
These deterministic fixture measurements exclude HTTP framing, injected display
agent bytes, catalogue metadata, and the temporary cleanup grace period; they
are not deployment traffic estimates. HTTP integration tests separately verify
live authorization and empty 304 responses for unchanged assets after an update.
