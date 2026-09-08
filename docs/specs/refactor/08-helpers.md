# 8. Small shared policies

Status: implemented. Inherits the full compatibility table in
[umbrella section 8](../../../SPEC-refactor.md#8-consolidate-small-shared-policies-and-helpers).
The checkpoint tree identity accessor row shipped separately (commit
`c3771e6`, "Share checkpoint content identity accessors across rendering and
row conversion") and is not revisited here. The remaining seven rows below
were each delivered as one commit on `refactor/helpers`.

## Scope and delivery

Use one reviewable change per policy below. Before extracting, inventory callers
and record differences that must remain explicit. Do not bundle these changes
with the catalogue or room concurrency migrations.

| Change | Required evidence |
| --- | --- |
| Checkpoint tree identity accessor | Legacy `sha` fallback and row conversion preserve distinct event identities. |
| Default main-file policy | Explicit names, format defaults, and unknown fallback remain unchanged. |
| Comment view conversion | Snapshot/event privacy and viewer-specific flags agree for author, owner, and other viewers. |
| Rendering publication helper | Reservation/upload/registration/cleanup stay consistent; current and historical validation retain their separate final checks. |
| UTF-16 helpers | Inventory room/session/CLI/text helpers; specify checked/clamped behavior, surrogate boundaries, overflow, and invalid ranges. |
| Request hashing | Shared byte hashing preserves receipts; canonicalization changes require byte-equivalence evidence or a versioned migration. |
| Bulk comment reconciliation | Identify remaining legacy/import/fixture/production callers; batch production queries if retained. |
| Unused fields/documentation | Verify `Peer.address` and `attach_deployment_lock_unavailable` across supported configurations before removal. |

## Acceptance

Retain focused regressions and test policy boundaries, including non-BMP and
combining characters for UTF-16, and nested keys, escapes, and numbers for request
digests. Do not reimplement already completed cleanup or write tests that merely
repeat the new helper's implementation. Rendering helper extraction must pass
the existing cancellation, authority, and physical-reclamation regressions.

## Implementation evidence

Each row below names its commit on `refactor/helpers` and the caller count
found before/after the change (a caller inventory was taken first, per the
scope note above, and none of these changes altered a persisted format, a
wire format, or a permission decision).

- **Default main-file policy** (`8bf23cc`). `main_path_for` moved from
  `room/mod.rs` into `document/render.rs`, beside `document_format`, its
  inverse. `room::main_path_for` is now a one-line delegating wrapper, so
  all 7 existing call sites (`room/mod.rs` x3, `server/documents.rs` x2,
  `seed/mod.rs` x2, `cli/publish.rs`, `tests/room.rs`) are unchanged.
  Explicit-name preservation, the four per-format defaults (including the
  R27 `main.tex` fix), and the unknown/empty-format `main.txt` fallback are
  byte-for-byte the same code, just relocated. `format_from_path` was not
  touched.

- **Comment view conversion** (`a02e4d8`). Added
  `CommentView::for_viewer(comment, author, is_owner)` in
  `room/comments.rs`; `comment_event_for`, `snapshot_for` and
  `snapshot_bundle` (3 call sites, all of `CommentView`'s construction
  sites) now call it instead of repeating the `mine`/`deletable`
  derivation. Added
  `tests::room::comment_view_agrees_across_snapshot_and_event_for_every_viewer`,
  which posts one comment and checks the snapshot and event paths agree for
  the comment's own author, the document's owner (a different account), an
  unrelated third party, and the anonymous/shared broadcast case.

- **Rendering publication helper** (`6ef7076`). `room/figures.rs`'s
  synctex/plain key derivation was duplicated 4 times
  (`catalog_rendering_size`, `put_rendering_as_authority`,
  `put_current_rendering_as_authority`, `read_rendering`); replaced all four
  with the existing `rendering_object_key` helper (previously used only by
  pruning). The abandon-on-failure sequence (release the accounted blob,
  recompute size) was duplicated 3 times; extracted as
  `Room::abandon_rendering`. The success-registration sequence (record size,
  stamp `rendering_written_at`) was duplicated 3 times; extracted as the
  free function `note_rendering`. This is pure deduplication: lock scopes,
  lock order, the early-return checks for an already-known rendering, and
  the current-tree digest/input-digest revalidation under the state lock
  are byte-for-byte unchanged, and the checkpoint-identified
  (`put_rendering_as_authority`) and current-tree-identified
  (`put_current_rendering_as_authority`) paths still do not call each
  other. `tests::renderings` (16/16), `tests::figures` (23/23) and
  `tests::room_figure_fixes` pass except
  `asset_uploaded_and_named_during_prune_survives`, which was confirmed to
  fail identically on the unmodified base commit under this machine's
  concurrent-build load (a 2s `tokio::time::timeout` on an unrelated
  asset-prune hook elapses) -- a pre-existing flake, not a regression.

- **UTF-16 helpers** (`113597e`). Inventoried every UTF-16 helper in
  `room/text.rs`, `document/session.rs`, `cli/export.rs`, `cli/suggest.rs`
  and `crates/text`; the inventory is now the module doc on
  `room/text.rs`. Two were byte-identical in behavior across modules
  (`len16`/`utf16_len`, and a clamped surrogate-safe unit-range slice used
  by both `room/text.rs`'s `apply_edit_str` and `cli/export.rs`'s inline
  copy); consolidated those into `room::text::{len16, utf16_slice}`
  (widened to `pub(crate)`) and pointed `cli/export.rs` at them, removing
  its 2 duplicate private functions. Every other helper stays separate,
  each with a documented reason: `apply_edit_str` clamps (a suggestion
  rehearsal against a possibly-stale merge base) where
  `document::session::apply_text_edits` must instead check and reject a
  whole batch (it writes the live `Y.Text`, where a clamped bad offset
  would be a wrong edit, not a safe one); `edit_text` deliberately chooses
  its retained-prefix/suffix boundary over `char`s rather than UTF-16 units
  specifically to avoid splitting a surrogate pair; the sticky-index
  functions use Yrs's own tracking, not string arithmetic; and the
  one-line uses in `cli::suggest` and `crates/text` are too small to name.
  Added 8 tests (`room::text::tests`) covering length and slicing across a
  non-BMP surrogate pair and a combining mark, a slice cut inside a
  surrogate pair (empty, not a panic), out-of-range and inverted ranges
  (clamped, not a panic), and `apply_edit_str`'s clamping behavior.
  `room::text` (8/8), `export` (19/19), `session` (14/14), and the
  suggest/suggestions suites (47/47) all pass.

- **Request digests** (`4dd6ea3`). `request_digest` (`room/catalog.rs`,
  backing comment/reply/suggestion-acceptance receipt idempotency keys)
  now calls `document::store::digest_of_bytes` for its final
  `hex(sha256(...))` step instead of inlining
  `hex::encode(sha2::Sha256::digest(...))`; this is the "reuse shared byte
  hashing" requirement. The canonicalization above it (key sorting, string
  escaping, number formatting) is untouched -- no byte-equivalence proof
  was attempted or needed, since nothing in it changed. Added
  `room::catalog::request_digest_tests` (4 tests) that fix specific hex
  digests for representative inputs (sorted/reordered object keys, string
  escapes including a literal newline, integer vs. float of equal value,
  nested array/object reordering), so a future canonicalization change
  that is not byte-compatible with stored receipts fails a test instead of
  silently breaking retry matching. `room::catalog::request_digest_tests`
  (4/4), `storage::catalog` (31/31), comment-related tests (34/34),
  `suggestions` (21/21).

- **Bulk comment reconciliation** (`cbd0b42`). Traced every caller of
  `Room::save` (the only path to the full-snapshot
  `save_catalog_comments`): all 7 call sites in `room/comments.rs` and
  `room/suggestions.rs` only reach it in their `catalog.get().is_none()`
  branch, and `accept_suggestion` refuses up front to run with a catalogue
  attached and no request id -- so no request-serving path reaches the
  full-snapshot reconciliation when a catalogue is present. The one
  production caller that does reach it with a catalogue attached is
  `seed::seed_annotations`, the `komodoc seed` dev command that writes a
  bounded, small, fixed set of demo annotations once per example document.
  Documented this on both `save_catalog_comments` and `Room::save`; no
  code change, since the caller is not a production request path and its
  input is bounded, so batching would add complexity it does not need.
  `tests::seed` (8/8) unaffected, as expected for a documentation-only
  change.

- **Unused fields and documentation** (`49e805b`). `Peer.address`
  (`room/mod.rs`) was confirmed write-only (`attach` stored it; nothing
  ever read it back, and the caller's own address is used independently
  for rate limiting in `server/socket.rs`) and removed, along with the
  `address` parameter of `Room::attach` and its 8 call sites (1 production,
  7 tests). `attach_deployment_lock_unavailable` was kept: its one caller
  (`tests::harness::server_over`) backs
  `tests::history::a_second_process_over_the_same_storage_does_not_write`,
  a real, tested, distinct configuration (a second local process over one
  bucket serving every room read-only) that production `serve` itself
  never takes (it exits rather than run a second writer) but that the
  in-process multi-server tests rely on; replaced the generic doc comment
  with one naming that caller and test, and kept `#[allow(dead_code)]`
  with a note that it is required only for a non-test library build, since
  its sole caller is `#[cfg(test)]` code. `room_lifecycle_fixes` (14/14),
  `room_suggestion_fixes` (13/13), `room_checkpoint_fixes` (6/6), `uploads`
  (10/10), `sockets` (6/6), and
  `tests::history::a_second_process_over_the_same_storage_does_not_write`
  all pass.

## Remaining limitations

None of the eight rows required a persisted-format or wire-format change,
so none needed a migration. The rendering-publication and UTF-16 rows found
genuine shared logic to extract; the bulk-comment-reconciliation and
unused-fields rows found their existing state (already restricted, or
already load-bearing) sufficient once documented, and made no functional
change beyond that documentation and, for `Peer.address`, a removal
confirmed safe by an exhaustive caller search.
