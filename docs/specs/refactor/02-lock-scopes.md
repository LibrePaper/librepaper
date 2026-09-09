# 2. Room and registry lock scopes

Status: implemented. Inherits [umbrella section 2](../../../SPEC-refactor.md#2-shorten-remaining-room-and-registry-lock-scopes).

## Scope and implementation

Split legacy comment persistence and catalogue-backed room mutations into
preparation under state, persistence without state, and generation-checked
completion. Use the [catalogue lifecycle](01-catalogue-execution.md) for cancelled
callers. A final in-memory check cannot substitute for fencing the durable write.

Serialize legacy comment writers with a dedicated operation gate and conditional
storage versions. Source edits must not require that gate. Completion applies
only the prepared operation's changes; failure cannot restore an old snapshot
over unrelated edits. Specify when each response, acknowledgement, and broadcast
is permitted relative to persistence.

For admission, collect candidates under the registry, release it before room
inspection, then revalidate identity, membership, and eviction eligibility before
removal. Account explicitly for temporary candidate `Arc` clones; preserve
protection for real active owners and in-flight checkpoints.

Before changing scopes, document the observed order of restore, checkpoint,
manifest, session, publication, rendering, assets, and any new comment gates,
room state, rooms/loading registries, and the catalogue journal gate. Audit purge,
restore, suggestion acceptance, and pruning against that order.

## Delivery and acceptance

Registry changes can land first. Migrate catalogue callers with track 1; legacy
comment persistence can be a separate change using the same lifecycle rules.

- Paused comment persistence permits source editing and socket activity.
- Concurrent comment writers preserve both changes; failure and cancellation
  preserve later state and connected-peer convergence.
- A stalled room does not block retrieval of an unrelated cached room.
- Deterministic purge/load and admission races never create two writable rooms,
  evict an actively owned room, or make all candidates busy due to scan clones.
- Record before/after lock hold measurements and complete the lock-order audit.

## Implementation evidence: registry portion

Capacity scans now use an admission gate separate from cached lookup. They
clone candidate identities under the rooms registry, estimate outside it, and
revalidate membership, active ownership, and idle state before removal. Final
state inspection under the registry uses `try_lock`, never an asynchronous wait.
One candidate clone plus the registry's owner is the only evictable owner count.

Registry lock order is admission -> rooms -> loading when reserving a load.
No path may acquire admission while holding rooms or room state. A load releases
the registries before taking its per-slug slot; it may take admission while
retaining that slot when publishing. Purge captures the slot under the registries
and drops them before awaiting it. Room estimates await state only outside both
registries; the final eviction state check is nonblocking. Cached lookups need
only rooms and bypass admission entirely.

Two deterministic lifecycle regressions poll cold admission until a held state
lock blocks it, then prove another cached lookup completes and a newly pinned
candidate survives eviction. These establish removal of registry hold time from
the entire blocked interval without a timing-only scheduling assumption.

## The lock order

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
loading slot out under rooms + loading and drops both before it fences,
`sweep` copies its candidates out the same way, and `erase_author_from_caches`
and `evict_idle` do too. And no catalogue job takes a room lock, so gate 15 is
a true leaf; that is what lets any of the awaits above release their gates
without changing what the catalogue sees.

`comment_write` is placed at 5 because it is acquired only immediately after
`restore_write` or on its own, and only the lease and room state are taken
while it is held. Nothing acquires `restore_write`, `publication_write` or any
checkpoint gate while holding it, so it cannot participate in a cycle.

Audited against that order:

- **Purge.** `purge_with_identity` reads the rooms and loading registries,
  drops both, fences, then takes `restore_write` → `checkpoint_write` →
  `manifest_write` → `session_write` → state. That is the same order restore
  and acceptance take, which is why fencing cannot deadlock against a restore
  already in flight. It never reacquires a registry while holding a room gate.
- **Restore.** `restore_and_checkpoint` takes `restore_write`, reads state and
  releases it, runs `checkpoint_impl` (`publication_checkpoint` read →
  `checkpoint_write`) to completion, and only then takes `assets_write` → state
  for the merge. Never inverted.
- **Suggestion acceptance.** `accept_suggestion` holds `restore_write` for the
  whole operation and takes state around each catalogue call, never across one.
  Its final outcome write now takes `comment_write` — after every checkpoint
  gate it used has been released, so 5 is never taken while 8 or 9 is held.
- **Pruning.** `prune_retained` holds `manifest_write` and calls
  `prune_renderings` (`rendering_write`) and `prune_assets` (`assets_write`)
  beneath it: 9 → 11 and 9 → 12, both forward.
- **Reservation guards.** `PendingEditReservation`, `ObjectChangeGuard`,
  `RoomWriteReservation` and `PublicationCheckpointToken` all settle through a
  `ReservationSlot`, whose `std::sync::Mutex` is held only for the length of an
  `Option::take`. `Drop` makes one synchronous catalogue call and holds no room
  gate while doing it: `PendingEditReservation` is now dropped with room state
  released on every path, which is what makes a cancelled edit's rollback
  unable to sit behind a reader.

No inversion was found. Nothing was deferred as out of scope.

## Implementation evidence: comment persistence and catalogue waits

`Room::save(&mut RoomState)` is gone. Every comment writer prepares an owned
mutation under state, persists with state released, and installs afterwards:
`apply_command`'s five mutating commands, `apply_suggestion_batch`,
`accept_suggestion`, `reject_suggestion`, and the seeding command through the
new `append_comment`. The legacy whole-blob write keeps its conditional
version — read under state, written through `write_owned`, so a lost
compare-and-swap still fences the room — and `comment_write` serialises the
writers so a prepared list differs from the room's by exactly its own change.
Seeding previously wrote the same object under no gate at all.

Failure semantics changed for the better. Each branch used to write its change
into room state, call `save`, and restore the fields it had overwritten if the
write failed; a change that arrived in between was lost. Nothing is installed
now unless storage took it, so a failed or cancelled write is invisible.

Response, acknowledgement and broadcast ordering: `apply_command` returns its
event only after the durable write returns and the caller broadcasts from that
return value, so no acknowledgement precedes persistence. The batch endpoint
broadcasts only the comments storage accepted, one event each. A refusal
returns an error to its sender alone and broadcasts nothing.

The gate-held map, updated from track 1's table. Rows not listed are unchanged.

| Migrated call | Gates held across the await |
| --- | --- |
| `RoomSet::sweep` → `touch_auto_checkpoint` | none (was `room.state`) |
| `Room::receive_update` → `reserve_pending_edit` | `publication_write` + `assets_write` (was + `room.state`) |
| `Room::write_session_inner` → `begin_room_write` | `session_write` (was + `room.state`) |
| `Room::record_size_now` → `checkpoint_stats` | none (was `room.state`) |
| `Room::apply_command` (seven calls) | `restore_write` + `comment_write` (was + `room.state`) |
| `Room::apply_suggestion_batch` (count + N inserts) | `restore_write` + `comment_write` (was + `room.state`) |
| `Room::reject_suggestion` | `restore_write` + `comment_write` (was + `room.state`) |
| `Room::accept_suggestion` outcome write | `restore_write` + `comment_write` (was `restore_write` + `room.state`) |
| legacy comment blob `swap` | `restore_write` + `comment_write` + the lease (was + `room.state`) |

Still holding room state across an await, deliberately:

- `put_rendering_if_current`'s publication. The identity check and the metadata
  write have to be one step or a rendering of a superseded tree can be marked
  current. This is the known legitimate case and is unchanged.
- `Room::write_owned` and `Room::put_accounted` object reservations, which are
  held under `session_write`, `publication_write` + `assets_write`, or
  `rendering_write` — the gates that serialise the object being written, not
  room state.
- `RoomSet::get`'s `document` lookup, under the per-slug `loading` slot. That
  slot is what makes a load exclusive; it is not room state and no cached
  lookup waits on it.

`receive_update` needed more than moving the await. Releasing state means the
document can move under the call — `restore_and_checkpoint` and suggestion
acceptance take `restore_write` rather than `publication_write`, so they are
not excluded — which would leave the size, file and encoded ceilings decided
against a document that no longer exists and the reservation sized for the
wrong snapshot. The session generation observed during admission is therefore
checked on reacquisition; a mismatch rolls the reservation back and decides
again, bounded at eight attempts and reported as saturation rather than as a
size refusal. Deletion and lost write authority are rechecked at the same
point, and a socket that closed or lost `may_edit` during the wait has its
bytes returned and its update ignored. The guarantee the umbrella asks for is
intact: nothing touches or relays the document until a reservation matching
the current generation is in hand.

`sweep`'s scheduler clock is the one place where releasing state admits a
stale observation. A document edited between the observation and the write has
its *automatic* interval deferred by one period; its quiet-period checkpoint,
a much shorter timer, still covers it. Holding state for a write the sweeper
makes once per interval per document was the worse trade.

### Validation

Five barrier regressions in `tests/room_lock_scopes.rs`, each verified to fail
when the corresponding hold is reintroduced (the reservation test times out on
the reader; the three comment tests deadlock or lose the concurrent edit):

- a paused legacy comment blob write permits a source edit, a socket
  attachment and a snapshot;
- two overlapping comment writers both land, in room state and in the stored
  object;
- a failed write installs nothing, the edit that arrived during it survives,
  and the next write lands;
- a cancelled write leaves no half-applied comment and releases its gate;
- a caller parked in the pending-edit reservation window holds neither room
  state nor anything a reader or socket attachment needs.

The two cancellation gates the room tests use are now one `ReservationGate`
that announces arrival, so a test acts inside the window instead of polling for
a side effect. The deterministic purge/load, admission, catalogue-room
cancellation, suggestion and retention suites are unchanged and green.

### Limitations

The three-phase legacy path clones the whole comment list per mutation. That
list is serialised in full by the write it feeds, so the clone is not the
dominant cost, but a room with the maximum 500 comments pays it on every
resolve. A per-row legacy format would remove it and is a persisted-format
change this track does not make.

`persist_comments` refuses rather than merges if the comment fence moved while
its write was in storage. Only a room reload can move it under the gate, and
the reloaded list is the durable one, so refusing is correct; there is no
regression covering it because no production path reloads a live room.
