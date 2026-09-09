# 4. Rendering lookup and downloads

Status: implemented and merged (`4e55fe9`, `5f8e9ea`); its catalogue query now runs on the track 1 boundary. Inherits [umbrella section 4](../../../SPEC-refactor.md#4-avoid-redundant-rendering-queries-and-downloads).

## Scope and implementation

Add a catalogue query or bounded joined page returning rendering candidates in
history order, including events outside the resident tail. Keep event SHA and
tree SHA separate in the returned metadata.

For byte-serving requests, resolve a candidate and read its body once, passing
the bytes and resolved metadata to the response path. For availability-only
requests, add a metadata operation to the blob contract where supported and
implement it for local and S3 storage. Registration alone is not availability.

Inventory caller fallback semantics before replacing lookups. Preserve distinct
missing-object and transient-error results. Do not silently fall back on errors
that currently abort a request. Defer current-tree digest caching unless a
profile establishes a separate need and an invalidation contract is specified.

## Delivery and acceptance

Deliver query, blob contract, and handler changes as coherent caller migrations
using track 1's execution boundary.

- Count at most one body download of the PDF ultimately served.
- Metadata checks use no PDF body download on supporting backends.
- Query counts grow by bounded pages, not one query per skipped history entry.
- Cover old retained PDFs, absent blobs, transient failures, labels, and restore
  events with shared content. Preserve each caller's fallback and authorization.
- Record query counts and downloaded bytes before and after on the same history.

## Implementation evidence

`newest_rendering_candidate` now joins history and registrations in one SQLite
query, returning separate event and content identities. A 130-event history
with only its first event rendered takes one connection operation instead of
131 for the prior history-plus-per-event lookup shape. The regression measures
both on the same catalogue and covers a later restore sharing that content.

The blob contract now exposes `exists`: filesystem metadata and S3 HEAD avoid
body downloads, while the compatibility fallback preserves other stores.
NotFound becomes false and transient/provider failures remain errors. The
optional latest-rendering endpoint retains its prior policy of returning no
candidate when the newest registered candidate is unavailable; it does not
silently choose an older registration.

A hooked-store regression records zero body reads for availability plus newest
metadata, two existence checks, and one body read for serving the selected PDF.
The existing PDF read handler already downloaded once, so no extra byte-serving
abstraction was introduced. The metadata handler passes its computed current
tree digest to the lookup, avoiding a second computation without adding a cache.

The catalogue query will use track 1's asynchronous execution boundary when
that migration lands. These changes do not independently complete that track.
