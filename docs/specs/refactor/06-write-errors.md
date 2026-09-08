# 6. Explicit room write errors

Status: implemented. Inherits [umbrella section 6](../../../SPEC-refactor.md#6-make-write-refusal-and-error-handling-explicit).

## Scope and implementation

Introduce typed distinctions for fenced/read-only, permission denied, quota
exceeded, not found, stale/conflicting input, invalid input, and storage failure.
Integrate permanent-size and temporary-capacity errors delivered by track 10.
Keep useful existing `AcceptError` distinctions and underlying logging context.

Convert silent mutators, including `set_main_file`, `add_text`, and `name_asset`,
to explicit results. Audit all publication, restore, rendering, server, and CLI
callers: stop dependent work on refusal and keep a valid empty CRDT update
distinct from an error.

Centralize HTTP/socket mapping while retaining wire fields, request IDs,
temporary IDs, and client-safe messages. Define status, retry policy, and cleanup
from error types. Preserve durable authority checks and legacy lease checks.

## Delivery and acceptance

Narrow error additions required by earlier correctness tracks ship with those
tracks. Complete the remaining caller migrations here.

- Read-only/refused mutations prevent dependent publication work.
- Quota errors retain their intended HTTP status and correlation fields.
- Changing error wording cannot alter status, retry, or cleanup behavior.
- Audit for remaining substring classification and silently ignored write results.
- Exercise storage failures and valid empty updates separately from refusals.

## Implementation evidence

Delivered on `refactor/write-errors`. The type is `room::WriteError`
(`crates/komodoc/src/room/error.rs`); the HTTP and socket mapping is
`server::reply::{refused, refused_with, socket_refusal}`.

### Before and after

Before, a refused write reached its caller as an empty `Vec<u8>` (`set_source`,
`set_main_file`), as nothing at all (`add_text`, `name_asset`), or as prose that
three handlers searched: `server/figures.rs` picked 507 vs 413 by
`why.contains("quota exceeded")` in two places and 403 by
`why.contains("actor rights")`, and `server/history.rs` did the same for
labelling. A refused publication source write was discarded: `handle_create`
called `set_main_file` and ignored the result, then registered the document and
committed the publication anyway.

After, every one of those paths carries a variant. `WriteError` covers
`ReadOnly(FenceReason)` (lease moved, deleted, unreadable state, non-authoritative
compatibility copy, permanently oversized), `PermissionDenied`, `Quota(QuotaKind)`,
`Size(SizeRefusal)` and `Capacity(CapacityRefusal)` (track 10's types, wrapped
rather than duplicated), `Figure(FigureLimit)`, `Document(DocumentLimit)`,
`RateLimited`, `ServerBusy`, `NotFound`, `Conflict`, `Invalid` and
`Storage(String)` whose context is logged and never sent to a client.
`AcceptError` is kept — `Stale` is what opens the merge editor — and converts
both ways (`From<WriteError> for AcceptError`, `From<AcceptError> for WriteError`),
so a handler classifies one kind of error.

### Mutators converted

`set_source`, `set_main_file`, `add_text`, `name_asset`, `put_asset` /
`put_asset_unlocked`, `put_rendering{,_as,_as_authority}`,
`put_current_rendering{,_as,_as_authority}`, `put_rendering_provenance{,_as_authority}`,
`label{,_as,_as_authority}`, `checkpoint`, `checkpoint_now`,
`checkpoint_publication_now`, `checkpoint_impl{,_locked}`,
`restore_and_checkpoint`, `write_manifest`, `write_session{,_inner}`, `persist`,
`rollback_publication_inner`, `put_accounted`, `save_catalog_manifest`,
`save_catalog_rendering{,_with_authority}`. `Applied::Refuse` now carries a
`WriteError` instead of a `&'static str`, and `Outgoing::Close` an owned reason.

A valid empty CRDT update stays an `Ok`: writing the source a document already
holds answers with an update, not with a refusal
(`tests::write_errors::an_empty_update_is_not_a_refusal`).

### Callers audited

- **Publication** (`server/documents.rs`): a refused `set_main_file` now aborts
  the publication, undoes the creation and answers from the error, instead of
  proceeding to `fill_directory`, the publication checkpoint and
  `commit_publication`. `fill_directory` returns `WriteError` and stops at the
  first refused `add_text`/`put_asset`/`name_asset`.
- **Restore and labelling** (`server/history.rs`): both answer through the
  central mapping; labelling keeps its `sha` correlation field, and the
  `actor rights` substring test is gone.
- **Rendering and figures** (`server/figures.rs`): all four substring
  classifications removed; the rendering reply keeps its `sha` field.
- **Onboarding** (`server/onboarding.rs`) and **seed** (`seed/mod.rs`): a
  refused starter/seed source write stops provisioning rather than
  checkpointing an empty document.
- **Sockets** (`server/socket.rs`, `server/mod.rs`): checkpoint failures answer
  with `socket_refusal`, which keeps `request_id`, `version` and `protocol`;
  a refused update closes the socket with the error's client message.
- **`komodoc sync`** (`cli/sync.rs`): `terminal_close_reason` asked whether a
  close reason was one of four literals. It now asks
  `room::error::permanent_close_reason`, the same table the room closes with,
  so rewording a refusal cannot leave the CLI reconnecting forever.
- **Suggestion decisions** (`room/suggestions.rs`): unchanged in shape; the one
  storage rollback path converts through `AcceptError::from`.

### Left as-is, with reasons

- `document/store.rs` still classifies `CatalogError::Conflict` prose into
  `PutError`. It is outside `server/` and `cli/` and already typed at its own
  boundary; converting it means touching the catalogue's construction sites,
  which two other tracks are editing concurrently.
- The catalogue's own refusal prose is read in exactly one place,
  `CatalogError::refusal()` in `storage/catalog/mod.rs`, which returns a
  `CatalogRefusal` value. `WriteError::from(CatalogError)` uses it; nothing
  above reads a message. `storage::catalog::refusal_tests` pins every message
  in that module to its class, so a rewording fails a test instead of silently
  moving a status code.
- The cached `read_only` flag is unchanged, and the durable catalogue status
  checks and legacy lease checks around it are untouched: `hold()` still
  verifies the lease on every durable write. `fence_reason` is only a
  companion recording *why* the existing flag was set.

### Status changes

Quota keeps its status: owner and deployment 507, upload rate 429, figure
ceilings 413, permission 403, not found 404, conflict 409. Two failure classes
move, deliberately: a genuine storage failure under a rendering or label write
answered 413 or 500 and now answers 503 with `retryable: true`, and a
publication whose source write is refused answers the refusal's status rather
than 201 followed by a document with no text.

### Tests

- `tests::write_errors::a_fenced_room_refuses_the_writes_a_publication_depends_on` —
  each publication step is refused, and nothing moves.
- `tests::write_errors::an_empty_update_is_not_a_refusal`.
- `tests::write_errors::a_storage_failure_is_told_apart_from_a_refusal` —
  injected blob failure; cause kept for the log, hidden from the client.
- `tests::write_errors::every_variant_maps_without_reading_its_message`.
- `tests::write_errors::no_handler_classifies_a_write_error_by_its_text` — the
  grep-style guard over `server/` and `cli/`; verified to fail when the old
  `err.contains("quota exceeded")` shape is reintroduced.
- `server::reply::refusal_tests` — wording independence of status and retry,
  correlation fields, storage context, deleted vs busy.
- `room::error::tests` — wording independence, quota statuses, catalogue
  conversion, fencing.
- `storage::catalog::refusal_tests` — every classified catalogue message.
- `tests::renderings::a_rendering_costs_the_owner_their_quota` and
  `tests::size_limits::*` continue to pass, the latter now asserting the
  variant rather than a substring of the refusal.

### Limitations

`WriteError::Document` and `ServerBusy` exist to keep four socket close
messages byte-identical, because a close frame carries only text and
`komodoc sync` reads it. If the protocol ever carries a machine-readable code
on close, those variants collapse into `Size`, `Quota` and `Capacity`.
