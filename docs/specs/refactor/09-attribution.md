# 9. Stable checkpoint attribution

Status: implemented and merged; schema 14 (13 became the concurrent comment-pass migration), writer paths, erasure
stages, and backup compatibility ship together. Inherits
[umbrella section 9](../../../SPEC-refactor.md#9-persist-stable-checkpoint-author-identities-for-erasure).

## Scope and implementation

Persist an optional stable account ID separately from display attribution.
Inventory every checkpoint-writing path, including user requests, automatic
checkpoints, restore, import, and system operations. Define which authenticated
identity each path carries, and leave anonymous/imported/system attribution
explicitly unresolved where no authoritative account association exists.

Deliver schema migration, row conversion, writer changes, erasure queries, and
backup/restore compatibility together. Never infer an account ID from a display
handle. Backfill only from authoritative associations and document unresolved
legacy records as an erasure limitation.

Erasure clears the stable ID and associated identifying display metadata in
bounded restartable batches. Check erasing-account state at the durable write
boundary so a queued checkpoint cannot reintroduce attribution. Coordinate that
transaction with track 1 if the execution migration has already landed.

Inventory attribution in immutable objects and backups. Define replacement,
retirement, and restore behavior for each retained copy, including how restore
avoids reintroducing erased identity. Do not claim complete erasure for copies
outside the implemented protocol. Preserve document content and event identity.

## Acceptance and delivery

- Cover renames, reused handles, equal display names on distinct accounts, and
  anonymous, imported, system, and unresolved legacy records.
- Restart interrupted migration and erasure batches without skipping records.
- Interleave checkpoint creation and erasure at the final durable check.
- Backup/restore preserves identity distinctions and obeys the documented
  erased-attribution policy.
- One account's erasure never changes another account's attribution or retained
  document content. State remaining legacy/immutable-copy limitations explicitly.

## Implementation evidence

Before: `checkpoints` carried only `by`, the mutable display string a writer
path happened to pass -- an owner key, a handle, a comment pseudonym, or
nothing. The erasure stage matched `by = <account id>`, so a renamed account's
checkpoints were unreachable and a reused handle could have matched rows that
were never that account's. `finish_erasure` counted the same mismatched
predicate, so it could declare an account free of attribution that still
carried it.

After: schema 14 adds a nullable `checkpoints.by_account` holding the stable
`accounts.id`, with a partial index on `(by_account, slug, sha)` for the
erasure keyset walk. There is deliberately no foreign key: a checkpoint may be
written where identities are not catalogued, and dropping attribution is the
erasure protocol's decision rather than a cascade's side effect. The column is
never serialized -- `document::history::Checkpoint::by_account` is
`#[serde(skip)]` -- so neither the history/timeline responses nor the legacy
`history/<slug>/index.json` object gained a field, and `by` displays exactly
what it did before.

### Writer path inventory

| Path | Identity carried | Stored `by_account` |
| --- | --- | --- |
| `server/socket.rs` `y-checkpoint` (`cli`/`sync`/`restore`/`label`) | socket viewer's authenticated account | account id, or NULL for a visitor/link-bounded caller |
| `server/socket.rs` last-editor-leaves (`left`) | same socket viewer | account id or NULL |
| `server/socket.rs` / `server/documents.rs` suggestion decision -> `accept` | deciding viewer (rechecked before the mutation) | account id or NULL |
| `server/mod.rs` comment checkpoint (`comment`) | commenter's viewer; display stays the pseudonym | account id or NULL |
| `server/figures.rs` rendering upload (`render`) | uploading viewer | account id or NULL |
| `server/history.rs` restore (`restore`, plus its `quiet` base) | viewer rechecked immediately before the room mutation | account id or NULL |
| `server/documents.rs` publication (`cli`) | publishing `Caller` | account id or NULL |
| `server/onboarding.rs` starter provisioning (`onboarding`) | the account signing in for the first time | account id |
| `room/mod.rs` tick, quiet and automatic | attribution of the last applied update, held in `Session::by` | account id when the last editor was signed in, else NULL |
| `room/mod.rs` tick, deferred request | the attribution the deferred request arrived with (`Session::asked`) | as the original request |
| `room/checkpoint.rs` `repair` (`recovered`) | none; nothing records who wrote a recovered object | NULL |
| `seed/mod.rs` seed/import (`cli`) | none; an operator ran a command | NULL |

`Attribution` is the only way a writer path supplies attribution. `From<&str>`
yields display-only attribution, so a bare string can never become an account
id; an empty account id is likewise recorded as unattributed.

### Erasure

`erase_account_batch` gained a `checkpoints_legacy` stage after the existing
`checkpoints` stage. `checkpoints` matches `by_account`; `checkpoints_legacy`
matches `by_account IS NULL AND by = <account id>`, which is what the old query
matched, so rows written before this change stay reachable. Both use the
existing keyset cursor shape `[slug, sha]`, so an erasure already in flight
resumes unchanged, and both clear only `by`/`by_account`: sha, tree_sha,
parent, seq, timestamps, size, label and content are retained. `finish_erasure`
counts both predicates.

All three durable insert sites (`insert_checkpoint`,
`insert_checkpoints_atomic`, and the publication commit in
`storage/catalog/operations.rs`) resolve attribution through
`attribution_for_insert` inside the insert transaction. A checkpoint admitted
and written before an erasure began therefore commits its content with
`by_account = NULL` and `by = 'Deleted user'` rather than reintroducing the
identity after its asynchronous object writes. `RoomSet::erase_author_from_caches`
additionally scrubs the resident manifest tail, `Session::by` and
`Session::asked`, which are what staged writes and timeline reads are built
from in a room that stays resident through the whole erasure.

### Backups and other retained copies

Local backups are a `VACUUM INTO` image of the catalogue, so `by_account` and
every erased row travel with it unchanged; a backup taken before an erasure
restores the attribution as it stood when the image was made, and restoring it
does not alter the live catalogue. A catalogue restored from an older backup is
migrated to 13 on open, with `by_account` NULL for every existing row. Remote
blob backups copy objects only; in catalogue mode the room writes no manifest
object, so no checkpoint attribution reaches them. The legacy object layout
(no catalogue) still writes `by` into `history/<slug>/index.json`; that layout
has no accounts table and no erasure worker, and is out of scope here.

### Tests

`storage/catalog/tests.rs`: `erasure_follows_the_account_not_the_handle`
(rename, reused handle, two accounts with equal display names),
`erasure_covers_legacy_rows_and_leaves_unattributed_ones_alone`
(anonymous/imported/system/legacy),
`erasure_batches_restart_without_skipping_checkpoints` (one row per batch with
periodic cursor loss), `a_queued_checkpoint_cannot_reintroduce_erased_attribution`,
`finish_erasure_waits_for_checkpoint_attribution`,
`interrupted_attribution_migration_restarts_and_backfills_nothing`, and
`vacuum_backup_preserves_the_identity_distinction`.

`tests/checkpoint_attribution.rs`:
`writer_paths_record_the_account_they_authenticated` (including that the served
JSON is unchanged), `a_display_string_never_becomes_an_account`,
`an_erasure_during_the_object_writes_wins_at_the_durable_boundary` (a blob-store
barrier holds the tree write while the erasure runs; no timing assertions),
`resident_room_caches_drop_the_erased_account`, and
`erasing_one_account_leaves_the_document_and_other_authors_alone`.

### Limitations

- Rows written before schema 14 have no `by_account`. Nothing in the catalogue
  associates them with an account, so the migration backfills nothing. They are
  reachable only through the `checkpoints_legacy` stage, and only when `by`
  literally holds the account id. A legacy row whose `by` is a handle cannot be
  erased; matching it by handle is exactly the failure this track removes.
- The `checkpoints_legacy` stage has no index to walk (the partial index covers
  only non-NULL `by_account`), so each of its batches scans the table. This is
  the cost the pre-existing stage already paid, and it disappears as legacy rows
  age out of retention.
- A prepared publication receipt stores its staged checkpoint descriptor,
  including `by` and `by_account`, in `catalog_operations.intent` until it
  commits or aborts. The durable insert that commits it drops erased
  attribution, but the transient receipt itself is not rewritten by an erasure
  batch. `begin_erasure` aborts prepared receipts on the erasing account's own
  documents; a receipt prepared by that account against a document it does not
  own can outlive the erasure stage until it resolves.
- Backups taken before an erasure retain the attribution. Restoring one
  reintroduces it in the restored deployment; there is no protocol here that
  replays erasures into existing images, and none is claimed.
