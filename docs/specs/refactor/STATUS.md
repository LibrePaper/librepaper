# Refactor implementation status

This checklist tracks the full scope of [the roadmap](../../../SPEC-refactor.md).
Every track's own spec file carries its implementation evidence, measurements
and remaining limitations; this table is the index. Integration evidence below
is for the combined branch, not for any track's branch alone.

| Track | State | Evidence or remaining work |
| --- | --- | --- |
| 1. Catalogue execution | Merged | Bounded `spawn_blocking` boundary sharing the one connection (`a8c244e`); every asynchronous production caller migrated in room/ (`62e855c`, `c3060ba`, `8bc0da4`) and outside it (`dd616ad`, `a230151`, `35f61a4`, `acea5e6`, `cf51a0a`); reservation lifecycle guards with service-owned completion; `Catalog::shutdown` wired into `serve`. Synchronous exceptions (seed, CLI admin, `backup.rs`'s own connection, `Drop` impls) are listed in the spec. |
| 2. Lock scopes | Merged | Registry scans (`9ec5959`); then `b63d131`, `7475c10`, `189480c`, `c360c2a`, `c329942`, `e8da8e8`: comment persistence split into prepare, persist and generation-checked apply under a dedicated `comment_write` gate, room state released around the edit reservation and the session, accounting and scheduler catalogue waits, and the complete lock-order audit with barrier regressions. |
| 3. Shared retention | Merged | `c3771e6`, `8ae9b94`, `c6e8a72`: one history traversal, four concurrent tree reads. |
| 4. Rendering lookup | Merged | `4e55fe9`, `5f8e9ea`: one joined candidate query; `exists` on the blob contract; now dispatched through the track 1 boundary. |
| 5. Resident estimates | Merged | `15c2279`, `00c3500`: `resident::Measured` caches comment and manifest sizes with `DerefMut` invalidation. 200 unchanged estimates of 1,000 comments: 3.84 s and 400 serializations before, 22 ms and 2 after, identical byte answer. |
| 6. Write errors | Merged | `06a4c4b`, `8404a4e`: `room::WriteError`, explicit results for every mutator, one HTTP/socket mapping keyed on variants, substring classification removed from handlers. |
| 7. Validated commands | Merged | `a86fbb7`, `24b0206`, `7237587`, `edf8d5f`. |
| 8. Shared policies | Merged | `c3771e6` plus `8bf23cc`, `a02e4d8`, `6ef7076`, `113597e`, `4dd6ea3`, `cbd0b42`, `49e805b`; per-row dispositions in the spec. |
| 9. Stable attribution | Merged | `6e7a6e0`, `2d8f005`: schema 14 `checkpoints.by_account`, `room::Attribution` through every writer, `checkpoints`/`checkpoints_legacy` erasure stages, attribution decided inside the insert transaction. |
| 10. Size limits | Merged | `3dfd689`, `322cdc4`, `dcc4c8b`, `3a25d82`, `70eb04d`: `PersistenceLimits` S/E/Q/M validated at startup, `--max-size` capped at 8 MB, encoded ceiling enforced before apply/broadcast, `JournalError::Busy` for capacity, `MemoryBudget`. |
| 11. S3 operations | Merged | `ee81c59`, `23d932a`: bounded retries with injected clock, byte-reconciled ambiguous writes, batch deletion with per-object outcomes; 250 requests to 1 for bulk retirement. |
| 12. Backup ownership | Merged | `c9a8719`, `65cd8b3`: `BackupOwnership` capability from the deployment writer lock; concurrent multi-host callers stay unsupported and are documented as such. |

## Integration evidence

Integration happened on `refactor/integration` and reached
`live-markdown-editor` only by fast-forward after a green full suite, because
the main branch was being committed to concurrently (the writing-assistant
work at `c812c70`, whose migration 13 forced attribution onto 14).

- At `9528f21` (tracks 8, 9, 10, 11, 12 and the track 1 machinery):
  879 library tests passed, 1 failed, 2 ignored; the failure was the known
  load-induced timeout in `asset_uploaded_and_named_during_prune_survives`,
  which passed three times alone in 0.3 s.
- At `dd4a696` (plus track 1 caller migration and track 6): 907 library tests
  passed, 0 failed, 2 ignored; all other workspace suites passed.
- Strict Clippy (`--all-targets --all-features -D warnings`) and `cargo fmt
  --check` were clean at every fast-forward.

Under heavy machine load about ten tokio-timeout tests fail with
`Elapsed(())`; each passes alone on an idle machine. Treat those as flakes
and rerun them individually rather than editing them.

## Review dispositions

Every branch was read in full by the parent before merging. Corrections made
at integration rather than sent back: two whole-list assignments in the
comment persistence split dereferenced through track 5's measured wrapper; three dropped apostrophes in attribution
comments; migration 13 renumbered to 14 with its schema assertions; the
size-limit tests adapted to the narrowed `Room::attach`; the two catalogue
caller halves unified on one lock-free job form, `execute_catalog`, after
each half had added its own under a different name; and a
`From<CatalogExecError> for WriteError` conversion so the room migration's
guards compose with the typed errors.

Known compromises, all recorded in the track specs: the catalogue's own
conflict prose is still classified by substring in exactly one pinned
function (`CatalogError::refusal`); `MaintenanceBorrow::drop` refunds through a
bare `spawn_blocking` outside admission; a room fenced by
the encoded-size backstop stays read-only until reopened.
