# 4. Rendering lookup and downloads

Status: proposed. Inherits [umbrella section 4](../../../SPEC-refactor.md#4-avoid-redundant-rendering-queries-and-downloads).

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
