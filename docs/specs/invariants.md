# Room and storage invariants

The guarantees that the room (`crates/librepaper/src/room/`) and storage
(`crates/librepaper/src/storage/`) code preserve, and the lock order every
room path follows. They were written down when the 2026-09 room and storage
refactor landed; the tests named in the code enforce them, and this page is
where they are stated together. Nothing here is a plan.

## Guarantees

- A `y-ack` acknowledges only a durably saved update, including updates saved
  through checkpoint and compensation paths.
- Retrying a prepared suggestion acceptance replays its stored CRDT state and
  cannot apply the proposal twice. Failed operations preserve unrelated edits
  and keep connected peers convergent.
- Deletion and loss of write authority fence subsequent durable mutations.
  Validation that happens before an asynchronous wait must not leave a window
  in which stale authorization permits a write.
- Room admission includes pending and in-progress writes. Cancellation and
  failure release reservations according to their ownership rules.
- Eviction cannot create a second writable instance while a request or
  checkpoint still owns the first instance.
- Pruning uses complete retained history, protects live and in-flight content,
  respects rendering grace periods, and skips unsafe deletion when its input
  cannot be read. The resident manifest is only a bounded cache.
- Event identity and tree content identity remain distinct: two history events
  can refer to the same content without becoming the same event.
- Persisted records, request digests, wire fields, and client correlation IDs
  stay compatible unless a change explicitly supplies a migration or a
  compatibility adapter.
- Physical reclamation releases accounting only after deletion is confirmed.
  Ambiguous writes retain conservative charges and durable recovery or cleanup
  records; a transient read error is not evidence that an object is absent.
- The catalogue-owned journal gate protects the current single-process local
  authority across graph reads, publication, recovery, and reclamation. It
  does not establish a cross-process or hosted reader protocol; adding such
  support requires a separate fencing and lease design.

## Lock order

Observed, not aspirational: every acquisition below was read out of the code
and each pair in the order is one some path actually takes. A gate may be
skipped; none may be taken out of order.

| | Gate | Taken by | What may be acquired while it is held |
| --- | --- | --- | --- |
| 1 | `RoomSet::admission` | capacity scans and load reservation | rooms, loading |
| 2 | `RoomSet::rooms` | every lookup | loading |
| 3 | `RoomSet::loading` | per-slug load slot | the per-slug slot itself |
| 4 | `restore_write` | restore, suggestion acceptance and rejection, comment commands, `edit_into_session`, publication checkpoints, purge | everything below |
| 5 | `comment_write` | every writer of the room's comment list | the lease and room state only |
| 6 | `publication_write` | socket edits, `set_main_file`, `add_text`, `name_asset`, source and tree reads, `edit_into_session` | everything below |
| 7 | `publication_checkpoint` | read by ordinary checkpoints and session writes, write by a publication and its rollback | everything below |
| 8 | `checkpoint_write` | `checkpoint_impl_locked`, purge | manifest, session, rendering, assets, state |
| 9 | `manifest_write` | manifest writes, labelling, `prune_retained` | session, rendering, assets, state |
| 10 | `session_write` | `write_session_inner`, purge | state |
| 11 | `rendering_write` | rendering publication, provenance, retirement | assets, state |
| 12 | `assets_write` | asset upload and pruning, socket edits, publication rollback | state |
| 13 | `Room::lease` | `hold`, on every durable write | state |
| 14 | `Room::state` | everything | nothing but the catalogue boundary |
| 15 | catalogue journal gate / execution boundary | every catalogue job | nothing: a job carries owned inputs and never a room guard |

Two rules make the order safe rather than merely observed. The registries are
always released before any room gate: `purge_with_identity` copies the room and
loading slot out under rooms + loading and drops both before it fences, and
`sweep`, `erase_author_from_caches` and `evict_idle` copy their candidates out
the same way. And no catalogue job takes a room lock, so gate 15 is a true
leaf; that is what lets any await above release its gates without changing
what the catalogue sees.

`comment_write` sits at 5 because it is acquired only immediately after
`restore_write` or on its own, and only the lease and room state are taken
while it is held. Nothing acquires `restore_write`, `publication_write` or any
checkpoint gate while holding it, so it cannot participate in a cycle.

Room state is still held across an await in three deliberate places: the
identity check and metadata write of `put_rendering_if_current`, which must be
one step or a rendering of a superseded tree can be marked current; the object
reservations in `Room::write_owned` and `Room::put_accounted`, held under the
gate that serialises the object being written; and `RoomSet::get`'s document
lookup under the per-slug loading slot, which no cached lookup waits on.

The barrier regressions in `crates/librepaper/src/tests/room_lock_scopes.rs`
each fail when the corresponding hold is reintroduced.

## Known compromises

- The catalogue's own conflict prose is classified by substring in exactly one
  pinned function, `CatalogError::refusal`.
- `MaintenanceBorrow::drop` refunds through a bare `spawn_blocking` outside
  admission.
- A room fenced by the encoded-size backstop stays read-only until reopened.
