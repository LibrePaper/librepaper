# 1. Catalogue execution and cancellation

Status: machinery implemented; the room caller group is migrated, the rest of
the caller migration is outstanding. The execution boundary, admission
budgets, lifecycle, counters and shutdown are in
`storage/catalog/execution.rs`; every catalogue call under `room/` now goes
through it, with the exceptions recorded in the room migration subsection
below. Callers in `journal/`, `maintenance`, `document/store.rs`, `server/`,
`seed/` and `cli/` are not migrated, so the track is not complete. Inherits
[umbrella section 1](../../../SPEC-refactor.md#1-move-catalogue-work-off-tokio-workers).

## Decision and boundary

Keep synchronous SQL off Tokio workers while preserving one connection's
transaction and temporary-reservation semantics. A dedicated database worker
owning that connection is the initial candidate: the existing connection is
already serialized, so ownership transfer is a natural fit. Compare it with a
bounded `spawn_blocking` adapter before committing to the architecture; choose
the simpler implementation that meets the same memory, lifecycle, and shutdown
contract. A thread by itself does not supply cancellation reconciliation or
remove database head-of-line blocking. Submit owned operations and
return owned results; execute each existing transaction as one request. Retain
the deployment writer lock and catalogue journal gate. The worker must not
perform object-store I/O or call back into async room code.

Inventory synchronous catalogue callers in room, journal, maintenance, server,
and CLI code. For every mutation record authority checks, receipt/operation ID,
reservation ownership, and reconciliation behavior. Do not assume every existing
catalogue method already has a durable receipt.

## Admission and lifecycle

The lifecycle below describes obligations, not a requirement to build a generic
durable job framework. Reuse existing operation identities and completion paths;
introduce new persisted state only for an identified unrecoverable transition.

Bound request count, retained input/result bytes, executing work, and waiting
producers. Obtain capacity before constructing large owned arguments. Small
descriptors may wait only under a bounded producer policy; otherwise return a
typed temporary saturation error. Assign finite result bounds or use pagination.
Specify numeric budgets and overload behavior before migrating callers.

| State | Owner and cancellation rule |
| --- | --- |
| Preparing | Caller owns staging and provisional permits; cancellation releases them. No SQL has been submitted. |
| Queued | Execution service owns request and permits. Cancellation may discard it only after atomically establishing that execution has not started. |
| Executing | Worker owns transaction and permits. Caller cancellation cannot interrupt ownership or imply rollback. |
| Committed or failed | Service records the result and schedules required completion/reconciliation, even if the response receiver has gone away. |
| Reconciled | Commit, rollback, or conservative recovery ownership is established; release only the permits and reservations whose obligations are settled. |

An SQL result is not necessarily the end of a room operation. Temporary edit
reservations need a service-owned completion path that either attaches the
reservation to the pending room mutation or releases that exact reservation
after proving it has no dependent write. Generation/operation identity must
prevent cleanup from releasing a newer reservation. After durable commit,
reconcile state and peer delivery without repeating the product mutation.

For receipt-backed operations, reconcile by the existing ID and digest. For
other operations, define an idempotent conditional reconciliation or add the
minimum required identity/receipt with compatibility handling. If the outcome
cannot be established, preserve conservative charges and recoverable ownership;
never retry a mutation merely because a response channel closed.

## Ordering, authority, and shutdown

Track 2 supplies room preparation/persistence/completion boundaries. Do not
hold room state or the global registry while waiting for worker capacity or SQL.
Document operation gates that remain held and their position relative to the
journal gate. No worker request may require a gate its waiting caller owns.
Perform final durable authorization/fence checks inside the relevant transaction.

Shutdown first stops admission, rejects queued work that has not started, then
settles executing work and service-owned completion. Document crash recovery
for committed work whose completion cannot run. Do not close SQLite or discard
cleanup ownership while accepted work still depends on it. Define behavior for
worker failure separately from caller cancellation.

## Acceptance and delivery

- A barrier blocking SQL does not block a Tokio heartbeat or socket work that
  requires no catalogue admission. An edit requiring a quota reservation may
  wait; it must not bypass the reservation to satisfy a responsiveness test.
- After the track 2 migration, waiting SQL retains neither room state nor the
  registry, so unrelated cached-room retrieval and independent socket activity
  progress. Test executor responsiveness and edit admission separately.
- Counters show finite queued/executing bytes, results, and waiting producers
  under saturation, including callers cancelled at each lifecycle transition.
- Inject cancellation immediately before dispatch, during SQL, after commit,
  and before room completion. Assert receipts, quota, temporary reservations,
  state, and peer convergence, not just returned errors.
- Exercise shutdown and worker failure with queued and committed requests.
  Record queue wait, execution duration, and outstanding reconciliation work.
- Audit all async production call sites; synchronous SQL remains behind the
  execution boundary. Preserve synchronous transaction-focused tests as useful.

Land the worker and lifecycle machinery first, then migrate caller groups with
their lock changes. Do not mark the track complete while request paths still
perform synchronous catalogue calls or rely on caller-owned cleanup after dispatch.

## Architecture decision: bounded `spawn_blocking`, not a database worker thread

The spec named a dedicated worker thread as the initial candidate. It was
compared against a bounded `spawn_blocking` adapter over the existing
connection mutex; the adapter was chosen. The two are equivalent on the
property that matters — synchronous SQL stops running on a Tokio worker — so
the comparison is about the cost each imposes elsewhere.

| Concern | Dedicated worker thread owning the connection | Bounded `spawn_blocking` over the shared mutex |
| --- | --- | --- |
| Memory | One request channel plus one thread stack (~2 MiB by default, or a tuned smaller stack). Identical per-request owned inputs. | No dedicated stack; blocking threads come from the pool the object store already uses. Identical per-request owned inputs. |
| Synchronous API during migration | Breaks it. The connection moves into the thread, so every remaining synchronous caller — the CLI, `seed`, the 46 direct `with_connection` sites, three `Drop` impls — must either round-trip through the worker (impossible from `Drop`, and a deadlock if a job itself calls one) or be migrated in the same change. That is the mass migration this delivery is explicitly not doing. | Keeps it. `execute` and `with_connection` both take the same mutex, so a caller can move to the async path independently of every other caller. |
| TEMP reservation table | Preserved, but only for callers that go through the worker. Any surviving synchronous caller would need a second connection and would silently fragment `room_edit_reservations`. | Preserved for both paths by construction: there is one connection and one TEMP table, and no second connection exists to fragment. |
| Lifecycle and cancellation | Needs an explicit request/response channel, a started flag, and a drain protocol. | Needs the same lifecycle rules; `spawn_blocking` supplies the "dispatched work is not cancellable" half for free, because dropping a `JoinHandle` does not cancel a blocking task. |
| Shutdown | Close the channel, join the thread; the connection drops with the thread. | Close admission, drain the executing counter, then take the connection out of the mutex. One extra `Option` behind the mutex. |
| Worker failure | A panic unwinds the thread and drops the connection with it, destroying the TEMP reservation table for the life of the process. Surviving that requires `catch_unwind` inside the thread anyway, plus a restart path that does not re-open the connection. | A panic unwinds a pool thread, not the connection: the connection stays in the `Catalog`. The only residue is a poisoned mutex, which is recovered on the next lock because rusqlite's `Transaction` guard has already rolled back. |
| Head-of-line blocking | Not improved. One connection is one connection; a long query blocks the next request either way. | Not improved, identically. Neither design removes it; both bound the backlog instead. |
| Migration cost | High: all-or-nothing. | Low: incremental, one caller group at a time. |

The thread buys nothing the adapter does not already provide and costs the
synchronous API, which this delivery must keep working. Neither design removes
database head-of-line blocking, so that is not a discriminator; if it ever
becomes the binding constraint the answer is read/write connection separation,
which is a separate design with its own TEMP-table problem.

Consequence recorded here because it is load-bearing: `lock_connection` now
recovers a poisoned mutex instead of reporting `Busy`. That is safe because
nothing but the connection lives behind the lock and an open transaction rolls
back through rusqlite's guard as the stack unwinds; refusing the lock instead
would discard live edit reservations for the rest of the process.

## Numeric budgets

Declared in `storage/catalog/execution.rs` and readable from tests.

| Budget | Value | Justification |
| --- | --- | --- |
| `MAX_ADMITTED_REQUESTS` | 64 | Queued plus executing. The connection is serial, so this bounds backlog, not parallelism: roughly a second of catalogue transactions, enough to absorb an editing burst and short enough that a saturated deployment reports pressure rather than hiding it. |
| `MAX_QUEUED_BYTES` | 16 MiB | Retained owned input bytes across all admitted requests: four maximum-size requests. Bounds the multiplication of large owned copies while one connection works through them. |
| `MAX_REQUEST_BYTES` | 4 MiB | Largest declared input for one request. Larger payloads belong in the object store, not in a queued SQL job. |
| `MAX_EXECUTING` | 2 | Jobs dispatched to blocking threads at once. One suffices for throughput; the second lets a job take the connection the instant the previous commit releases it without letting the backlog occupy the shared blocking pool. |
| `MAX_WAITING_PRODUCERS` | 128 | Past this, waiting is itself the unbounded queue the budgets exist to prevent. |
| `SMALL_REQUEST_BYTES` | 4 KiB | A request at or below this may wait for capacity; a larger one fails fast, because a producer parked holding a multi-megabyte owned input is exactly the memory the budget bounds. |

Overload behaviour: `reserve_execution` returns `CatalogExecError::Saturated`
when a large request finds no capacity, or when a small one arrives with
`MAX_WAITING_PRODUCERS` already waiting. It returns `TooLarge` above
`MAX_REQUEST_BYTES`, and `ShuttingDown` once admission has stopped. All are
temporary-and-safe: nothing was submitted and no SQL ran, so a caller may
retry or shed the request. `Saturated` is counted separately from failures.

## Implemented boundary

Reserve-then-submit, so capacity is obtained before the caller builds the
owned arguments it will hand over:

- `Catalog::reserve_execution(input_bytes) -> CatalogReservation` takes the
  request, byte and (at submit) executing permits. Holding it is the
  `Preparing` state: the caller owns it and dropping it releases everything.
- `CatalogReservation::execute(job)` submits `FnOnce(&mut Connection)`,
  mirroring `with_connection`; `::transaction(job)` submits
  `FnOnce(&Transaction)`, mirroring `immediate`, so a whole transaction is one
  request and is never split across queued statements.
- `::execute_with_completion` / `::transaction_with_completion` attach a
  service-owned `CatalogCompletion` hook. It runs on the executing thread
  after the job, with the connection and a `CatalogOutcome` (`Committed`,
  `Failed`, `Panicked`, `Rejected`), before the request's permits are
  released, and it runs whether or not the caller is still waiting. A closure
  of the right shape implements the trait. This is the attachment point for
  the reservation reconciliation the caller migration will need; no durable
  job framework is introduced.
- `Catalog::execute` / `::execute_transaction` are one-step conveniences for
  callers whose inputs are already small and owned.

Lifecycle, per the table above: cancelling before dispatch (dropping the
reservation, or dropping the submit future while it waits for an executing
permit) releases the permits and runs no SQL. Once dispatched the service
owns the request — dropping the `JoinHandle` does not cancel a blocking task —
so cancellation cannot interrupt the transaction or imply rollback, the result
is recorded in the counters, the completion hook still runs, and a job is
never re-run because a response channel closed.

Ordering and authority are unchanged. Jobs must not perform object-store I/O,
call back into async room code, or require a gate their waiting caller holds:
a job runs on a blocking thread with no reactor, and a caller blocked on a gate
that a queued job needs would deadlock the single connection. The catalogue
journal gate and the deployment writer lock are untouched and are still taken
by callers outside this boundary.

Shutdown (`Catalog::shutdown`) stops admission and closes the three
semaphores, so waiting producers and queued-not-started requests get
`ShuttingDown`; a request already dispatched but not yet started sees the
shutdown flag at the top of the blocking closure and is rejected without
running SQL, with its completion hook invoked as `Rejected`. Executing work
keeps the connection until its transaction and its hook finish, because
interrupting there would produce exactly the uncertain outcome the lifecycle
rules exist to avoid. SQLite is closed only after the drain; afterwards every
path, synchronous or asynchronous, returns `CatalogError::Closed`. Crash
recovery for committed work whose completion could not run is unchanged: the
TEMP reservation table dies with the process, so no stale edit reservation
survives a kill, and durable work is reconciled by its existing receipt.

Worker failure is defined separately from caller cancellation. A panicking job
returns `CatalogExecError::Panicked` to its caller and increments the
`panicked` counter; the blocking thread is not the connection's owner, so the
connection, its rolled-back transaction and the TEMP reservation table all
survive, and the next request uses the same connection. A panicking completion
hook is caught the same way.

## Diagnostics

The deployment exposes no metrics endpoint or status route, so
`Catalog::execution_snapshot() -> CatalogExecutionSnapshot` is the exposure:
`queued`, `executing`, `queued_bytes`, `waiting_producers`, `admitted`,
`completed`, `failed`, `panicked`, `rejected`, `saturated`, and separate
totals and maxima for queue wait and execution duration in microseconds.
Queue wait is measured from reservation to the start of execution, execution
duration from the start of the job to its return, so a slow queue and a slow
query are distinguishable.

## Call-site inventory

The map the caller migration follows. Built by enumerating the 142
`pub fn`/`pub(crate) fn` names on `impl Catalog` across the ten catalogue
files, resolving every call in `crates/komodoc/src` outside `storage/catalog/`
whose receiver chain roots in a `catalog` binding, excluding `#[cfg(test)]`
regions and `tests/`, then reading each enclosing function for its gate,
receipt and reservation ownership. **228 production call sites in 18 files.**

Gate vocabulary: room gates (`room/mod.rs:282-323`) `restore_write`,
`publication_write`, `publication_checkpoint`, `checkpoint_write`,
`manifest_write`, `rendering_write`, `assets_write`, `session_write`, and
`state` (`Mutex<RoomState>`); `RoomSet` gates `admission`, `rooms`, per-slug
`loading`; `catalog.journal_gate`, one `tokio::sync::Mutex<()>` shared by
`JournalRuntime.publication`, `DeletionWorker.journal_gate` and
`JournalRetirementWorker.retirement_gate`; the deployment `writer.lock` file.
Every row additionally takes `Catalog.connection` internally, so that is not
repeated.

### room/ — 79 sites

| File | Function (async?) | Catalogue method(s) | Receipt/operation id | Reservation ownership | Lock/gate held |
| --- | --- | --- | --- | --- | --- |
| room/mod.rs:395 | `RoomWriteQuota::drop` (sync, Drop) | `finish_room_write` | none | releases the room-write reservation | none; runs at unwind |
| room/mod.rs:675 | `RoomSet::get` (async) | `document` | none | none | per-slug `loading` slot |
| room/mod.rs:891 | `RoomSet::sweep` (async) | `documents_due_auto_checkpoint` | none | none | none |
| room/mod.rs:906 | `RoomSet::sweep` (async) | `touch_auto_checkpoint` | none | none | `room.state.lock()` |
| room/mod.rs:1521-1562 | `Room::write_owned` (async) | `reserve_object_change`, `commit_object_change`, `abort_object_change` ×3 | local `operation_id`, not a catalogue receipt | object reservation for one mutable key | `session_write`, or `publication_write`/`assets_write` via `save` |
| room/mod.rs:1588-1608 | `Room::put_accounted` (async) | `reserve_object_change{,_with_authority}`, `commit_object_change`, `abort_object_change` ×3 | local `operation_id`; optional `MutationAuthority` | object reservation for one asset/rendering key | `rendering_write` or `assets_write` |
| room/mod.rs:1972, 1990 | `Room::receive_update` (async) | `reserve_room_edit` ×2 (reserve, rollback) | none | **owns the room-edit reservation** | `publication_write` + `assets_write` + `state.lock()` |
| room/mod.rs:2094 | `Room::write_session_inner` (async) | `begin_room_write` | none | **takes the room-write reservation** (`RoomWriteQuota`) | `session_write` + `state.lock()` |
| room/mod.rs:2153 | `Room::write_session_inner` (async) | `finish_room_write(.., true)` | none | commits it | `session_write` |
| room/checkpoint.rs:50 | `PublicationCheckpointToken::drop` (sync, Drop) | `refund_checkpoint_token` | none | refunds its checkpoint-budget bucket | none |
| room/checkpoint.rs:89 | `Room::reserve_publication_checkpoint` (sync fn) | `admit_checkpoint_token_with_limits` | none | **takes the checkpoint budget token** | none |
| room/checkpoint.rs:247-664 (10) | `Room::checkpoint_impl_locked` (async) | `checkpoint` ×2, `admit_checkpoint_token_with_limits`, `document`, `stage_publication_checkpoint`, `touch_auto_checkpoint` ×2, `checkpoint_stats`, `shed_checkpoints_to_limits`, `delete_checkpoint` | stages into `document.pending_publication`; no operation id | may hold or admit a `PublicationCheckpointToken` | `checkpoint_write`; callers add `publication_checkpoint`, `restore_write`, `publication_write`. `state.lock()` is taken and released around each call |
| room/checkpoint.rs:809, 827, 856 | `checkpoint_by_sha`, `checkpoints_prefix`, `checkpoint_page` (async) | `checkpoint`, `checkpoints_prefix`, `checkpoints` | none | none | none |
| room/checkpoint.rs:973-994 | `Room::label_as_authority` (async) | `checkpoint`, `label_checkpoint{,_with_authority}` | `MutationAuthority`, no operation id | none | `manifest_write` |
| room/checkpoint.rs:1448 | `Room::record_size_now` (async) | `checkpoint_stats` | none | none | `state.lock()` held across the call |
| room/catalog.rs:14, 36 | `load_catalog_comments` (sync) | `comments`, `replies` | none | none | none |
| room/catalog.rs:172-225 (7) | `save_catalog_comments` (sync) | `comment`, `update_comment`, `insert_comment`, `replies`, `update_reply`, `insert_reply`, `delete_reply` | none; whole-snapshot legacy path | none | called from `Room::save(&mut RoomState)`, so `state.lock()` plus the caller's `restore_write` |
| room/catalog.rs:249 | `load_catalog_manifest` (sync) | `checkpoints_tail` | none | none | none |
| room/catalog.rs:273 | `load_catalog_checkpoint_rows` (sync, paged loop) | `checkpoints`, 200/page | none | none | none, or `manifest_write` via `prune_retained` |
| room/catalog.rs:318-330 | `save_catalog_manifest` (sync) | `document`, `stage_publication_checkpoint`, `insert_checkpoints_atomic` | branches on the `pending_publication` receipt | none | `manifest_write` (+ `checkpoint_write`) |
| room/catalog.rs:375-419 | `save_catalog_rendering_with_authority` (sync) | `rendering`, `publish_rendering{,_with_authority}` | `MutationAuthority`, no operation id | object reservation already committed | `rendering_write`; also under `state.lock()` from room/figures.rs:522 |
| room/figures.rs:74, 278, 559, 641 | `catalog_rendering_size`, `has_rendering`, `read_rendering`, `read_rendering_provenance` (async) | `rendering` | none | none | `rendering_write` for the first; none for the rest |
| room/figures.rs:109 | `delete_accounted_blob` (async) | `release_object_accounting_key` | none | releases committed object accounting | `rendering_write` |
| room/figures.rs:318, 668 | `rendering_sha`, `newest_rendering_for` (async) | `checkpoint`, `newest_rendering_candidate` | none | none | none |
| room/figures.rs:773 | `prune_renderings` (async) | `retire_rendering` (loop) | none | none | `manifest_write` + `rendering_write` |
| room/comments.rs:540-932 (7) | `Room::apply_command` (async) | `pending_suggestion_accept` ×2, `update_comment` ×2, `delete_comment`, `insert_reply_request`, `insert_comment_request` | `request_id` + `request_digest` on the two `*_request` inserts only | none | `restore_write` + `state.lock()` held across all seven |
| room/suggestions.rs:220-514 (8) | `Room::accept_suggestion` (async) | `begin_suggestion_accept`, `suggestion_accept_checkpoint`, `finish_suggestion_accept` ×2, `suggestion_accept_update`, `checkpoint`, `stage_suggestion_accept_update`, `record_suggestion_accept_checkpoint` | full receipt: `request_id` + `acceptance_digest` | owns the suggestion-accept staging record | `restore_write`; `state.lock()` released around each call |
| room/suggestions.rs:567, 598 | `Room::reject_suggestion` (async) | `pending_suggestion_accept`, `update_comment` | none | none | `restore_write` + `state.lock()` |

### room/ migration: job shapes, reconciliation, and gates still held

Every row above is migrated except the four recorded as remaining synchronous
at the end of this subsection. The room's catalogue calls are now owned jobs
submitted through the boundary; the helper functions that build them live in
`room/catalog.rs`, and `room/checkpoint.rs` owns the checkpoint budget token.

One addition to the boundary was required. A job submitted with `execute` or
`transaction` is given the connection, and the connection mutex is not
reentrant, so such a job cannot call a catalogue method: the room would have
had to reimplement forty methods as raw SQL. `CatalogReservation::execute_catalog`
(and `Catalog::execute_catalog`, `execute_catalog_with_completion`,
`CatalogServiceCompletion`) submits a job that is handed the catalogue itself
with the lock free. Each catalogue method inside such a job still opens and
commits its own `BEGIN IMMEDIATE` transaction exactly as it does today, so the
migration neither splits an existing transaction nor silently fuses two of
them; what changes is only that the wait happens on a blocking thread under
bounded admission. Where a room path made several catalogue calls in a row,
they are one job — one admission, the same sequence of transactions.

`room_edit_reservations` gained a `generation` column. It is a process-local
TEMP table recreated at every open, so this is not a persisted-format change.
Every write to a row advances it, and `Catalog::restore_room_edit` undoes a
reservation only while the generation it wrote still stands. That is the
identity the lifecycle rules ask for on an operation that has no receipt: it
makes cleanup unable to release a newer reservation, and makes a repeated
rollback a no-op rather than a second release of the same quota.

Reservation lifecycle. Three room reservations are held across an await and so
need a settlement path for a caller that disappears:

| Reservation | Taken by | Service-owned settlement | Identity it reconciles by |
| --- | --- | --- | --- |
| Pending room edit (`reserve_room_edit`) | `Room::receive_update` | `EditReservationCleanup` hook releases it when the caller was already gone as the transaction settled; `PendingEditReservation::drop` releases it when the caller goes away after that, in the window the hook cannot see | the reservation's generation |
| Object change (`reserve_object_change`) | `Room::write_owned`, `Room::put_accounted` | `ObjectChangeCleanup` hook aborts the operation when its caller is gone; `ObjectChangeGuard::drop` aborts it if the caller is cancelled later, including during the object-store upload | the operation id |
| Session write (`begin_room_write`) | `Room::write_session_inner` | `RoomWriteCleanup` hook, and `RoomWriteReservation::drop`, settle it as a failed write so the bytes return to the room's pending reservation | the session writer gate serialises the row, so no newer owner can exist |
| Checkpoint budget token | `Room::checkpoint_impl_locked` | `CheckpointTokenCleanup` hook refunds the admitted bucket when its caller is gone; `PublicationCheckpointToken::drop` refunds it otherwise | the exact owner and hour SQLite admitted |

The handshake is one shared `ReservationSlot`: the job records what it took,
and exactly one of the completion hook and the guard's `Drop` takes it back
out. The hook covers `Executing` and `Committed`, because it runs on the
executing thread before the result is delivered; `Drop` covers everything
after that, and is the one catalogue call the room still makes synchronously,
because a `Drop` cannot await. Two windows the previous code did not settle
at all are now settled: a caller cancelled while `reserve_room_edit` was in
SQL, and a caller cancelled during the object-store upload between
`reserve_object_change` and its commit — the latter previously leaked the
reservation for the life of the process.

Operations with no receipt, and the reconciliation rule each was given:

- Checkpoint labels, rendering publication, comment resolve/reopen/anchor and
  suggestion rejection are *absolute* writes: the row is set to exactly what
  the request asked for, so repeating one is the same write and a cancelled
  caller leaves either the old value or the new one. What they needed was not
  a receipt but an ordering rule, and they now have it: the room applies the
  change to its own state only after the write returns, so a cancelled caller
  can no longer leave this room showing a decision no row records and no peer
  was told about. Comment insert, reply insert and suggestion acceptance keep
  their existing `request_id`/digest receipts and need nothing further.
- Shed-checkpoint deletion reports which rows it actually removed, and only
  those objects are deleted, exactly as before.

Gates still held across the await, for track 2's map. This delivery
deliberately did not reorder or shorten any lock; a job runs on a blocking
thread, so the caller's await no longer parks a Tokio worker, but the gates
below are still held for the whole call.

| Migrated call | Gates held across the await |
| --- | --- |
| `RoomSet::get` → `document` | per-slug `loading` |
| `RoomSet::sweep` → `documents_due_auto_checkpoint` | none |
| `RoomSet::sweep` → `touch_auto_checkpoint` | `room.state` |
| `Room::load`/`load_session` → comments, manifest | none |
| `Room::save` → `save_catalog_comments` | `room.state` (the caller's `&mut RoomState`), plus the caller's `restore_write` |
| `Room::write_owned` object reserve/commit/abort | `session_write`, or `publication_write` + `assets_write` via `save` |
| `Room::put_accounted` object reserve/commit/abort | `rendering_write` or `assets_write` |
| `Room::receive_update` → `reserve_pending_edit` | `publication_write` + `assets_write` + `room.state` |
| `Room::write_session_inner` → `begin_room_write` | `session_write` + `room.state` |
| `Room::write_session_inner` → `finish_room_write` | `session_write` |
| `Room::checkpoint_impl_locked` → duplicate check, budget admission, stats, staging, shed, delete | `checkpoint_write`; callers add `publication_checkpoint`, `restore_write`, `publication_write`. `room.state` is taken and released around each, as before |
| `Room::record_size_now` → `checkpoint_stats` | `room.state` |
| `Room::label_as_authority` → checkpoint read, `label_checkpoint` | `manifest_write` |
| `Room::write_manifest` → `save_catalog_manifest` | `manifest_write` (+ `checkpoint_write`) |
| `Room::figures` rendering reads and publication | `rendering_write` or `assets_write`; the publication in `put_rendering_if_current` additionally holds `room.state`, which is deliberate — it is what keeps a stale PDF from being marked current |
| `Room::prune_renderings` → `retire_rendering` | `manifest_write` + `rendering_write` |
| `Room::prune_retained` → `load_catalog_history` | `manifest_write` |
| `Room::apply_command` (seven calls) | `restore_write` + `room.state` |
| `Room::accept_suggestion` (eight calls) | `restore_write`; `room.state` is released around each |
| `Room::reject_suggestion` | `restore_write` + `room.state` |

`receive_update` needed one behavioural change to become awaitable at all: a
parsed `yrs::Update` is not `Send`, so it cannot be held across the
reservation's await inside a spawned socket task. The parse is therefore
dropped after the size and file ceilings have decided and repeated once the
bytes are reserved. Repeating it is cheap beside the full document encode the
same path already performs to size the reservation, and it is the alternative
to reserving before those ceilings have decided, which would charge — however
briefly — for updates the room refuses.

Remaining synchronous catalogue calls in `room/`, and why:

- `Room::reserve_publication_checkpoint` (`admit_checkpoint_token_with_limits`).
  It is a synchronous `pub fn` called from `document/store.rs` and the CLI;
  making it asynchronous changes callers outside `room/`, which belongs to the
  delivery that migrates them. It is on the publication request path.
- The three `Drop` impls: `PendingEditReservation`, `ObjectChangeGuard`,
  `RoomWriteReservation` (and `PublicationCheckpointToken`). A `Drop` cannot
  await. Each runs only on a cancellation or failure path, each is a single
  conditional statement, and the alternative is leaving quota charged for work
  that will never happen.

### server/ — 16 sites

| File | Function (async?) | Catalogue method(s) | Receipt/operation id | Reservation ownership | Lock/gate held |
| --- | --- | --- | --- | --- | --- |
| server/mod.rs:370-427 | `Server::authenticated_identity` (async) | `account` ×2, `upsert_account` ×2 | none | none | none |
| server/mod.rs:695 | `Server::clear_dead_session` (sync fn) | `account` | none | none | none |
| server/onboarding.rs:48, 56, 117 | `initialize_account_examples` (async) | `pending_account_examples` ×2, `complete_account_example` | none | none | `self.onboarding` mutex for the last two |
| server/routes.rs:219 | `/api/account/erase` (async) | `begin_erasure` | new erasure generation; no operation id | none | none |
| server/serve.rs:217, 232, 236 | `serve` startup (async) | `totals`, `set_link_sealing_key`, `add_link_decryption_key` | none | none | deployment `writer.lock` |
| server/serve.rs:397 | maintenance ticker (async) | `prune_checkpoint_budgets` | none | none | none |
| server/sharing.rs:559, 583 | `handle_transfer` (async) | `upsert_account`, `transfer_ownership_authorized_with_generation` | caller id + `session_generation` re-checked inside the write | none | none; relies on SQLite's write lock |
| server/signin.rs:132 | `sign_in` (async) | `upsert_account` | none | none | none |

### document/store.rs — 50 sites

| File | Function (async?) | Catalogue method(s) | Receipt/operation id | Reservation ownership | Lock/gate held |
| --- | --- | --- | --- | --- | --- |
| store.rs:610 | `reserve_object_bytes` (sync fn) | `reserve_document_bytes_with_authority` | `MutationAuthority`; no operation id | **takes a document-bytes reservation** | none |
| store.rs:659, 675, 684 | `admit_replacement_upload`, `release_object_bytes`, `begin_delete` (sync fn) | `admit_document_upload`, `release_document_bytes`, `begin_delete` | none | upload admission; releases the above | none |
| store.rs:720-780 (5) | `open_with_catalog` (async, startup) | `pending_publications`, `operation`, `commit_operation`, `abort_operation`, `discard_aborted_creation` | operates on `(storage_id, request_id)` receipts | resolves publication reservations left by a crash | deployment `writer.lock` |
| store.rs:848, 860 | `pending_publication{,_result}` (sync fn) | `document` | reads the `pending_publication` id | none | none |
| store.rs:920 | `visible_page_with_options` (sync fn) | `visible_documents_with_examples` + N× `load_catalog_entry` | none | none | none |
| store.rs:959, 986 | `read_source`, `read` (async) | `document` | none | none | none |
| store.rs:1177-1192 | `prepare_publication` (async) | `document`, `operation`, `prepare_operation` | **creates the receipt**: `request_id`, `request_digest`, intent, actor | opens the publication operation | none |
| store.rs:1252 | `reserve_publication_peak` (sync fn) | `reserve_publication_peak` | none | **takes the publication peak reservation** | none |
| store.rs:1261-1284 | `commit_publication`, `abort_publication` (async) | `document`, `commit_operation`/`abort_operation` | the `pending_publication` receipt | closes/releases it | none |
| store.rs:1295-1444 (5) | `put_catalog` (async) | `document` ×2, `upsert_account`, `replace_document_admitted`/`create_document_admitted` | none passed in | **takes creation/replacement admission plus initial peak reservation** | none while calling |
| store.rs:1495-1508 | `record_history` (async) | `document`, `stage_publication_measurement`, `record_document_measurement` | branches on and stages into the `pending_publication` receipt | none | caller holds `checkpoint_write` |
| store.rs:1579, 1588 | `rename` (async) | `document`, `update_document` | none | none | none |
| store.rs:1738-1761 | `adopt` (async) | `upsert_account`, `documents`, `transfer_ownership` (loop) | none | none | none |
| store.rs:1826 | `room_for` (async) | `documents` (full scan) | none | none | none |
| store.rs:1952-2024 (5) | `remove` (async) | `document`, `begin_delete`, `queue_delete` (loop), `complete_delete_object` (loop), `finish_delete` | none | drives the delete lifecycle; runs an inline retirement pass that takes `journal_gate` | none held by `remove` itself |
| store.rs:2283 | `catalog_entries` (sync fn) | `documents` + N× `load_catalog_entry` | none | none | none |
| store.rs:2300, **2307**, 2356 | `load_catalog_entry` (sync fn) | `document`, **`with_connection`**, `open_link_key` | none | none | **`open_link_key` is called from inside the `with_connection` closure: a re-entrant catalogue call while the connection mutex is held** |
| store.rs:2424-2522 (5) | `update_catalog_entry_access` (sync fn) | `document`, `links`, `seal_link_key`, `upsert_account` (loop), `update_document_access` | optional `MutationActor` re-checked in the final write | none | none; a read-modify-write across five separate calls |

### storage/journal/ — 38 sites

| File | Function (async?) | Catalogue method(s) | Receipt/operation id | Reservation ownership | Lock/gate held |
| --- | --- | --- | --- | --- | --- |
| journal/store.rs:81 | `reconcile_object_reservations` (async) | `with_connection` | reads `operation_id` rows | reconciles journal object reservations | `journal_gate` |
| journal/store.rs:142-184 | `reserve_object`, `commit_object`, `abort_object` (sync fn) | `slug_by_storage_id`, `reserve_object_change`, `commit_object_change`, `abort_object_change` | `operation_id` from the runtime | **the journal object reservation** | `journal_gate` |
| journal/store.rs:197, 521 | `initialize`, `initialize_local` (sync fn) | `configure_journal`, `journal_state` | none | none | writer lock |
| journal/store.rs:211 | `retire_storage` (sync fn) | `with_connection` (IMMEDIATE) | none | none | `journal_gate` |
| journal/store.rs:481 | `compaction_due` (sync fn) | `with_connection` | none | none | none |
| journal/store.rs:761-783 | `release_compaction_borrow` (sync fn) | `slug_by_storage_id`, `with_connection`, `release_maintenance` | `maintenance-{operation_id}` | **releases the maintenance reservation** | `journal_gate` |
| journal/store.rs:796-867 | `resolve_committed_preparation`, `resolve_preparation`, `abort_preparation` (sync fn) | `with_connection` | `operation_id` | resolves the journal preparation | `journal_gate` |
| journal/store.rs:978-1070 | `state`, `prepare`, `commit_segments` (sync fn) | `with_connection` | journal `operation_id` | journal head reservation | `journal_gate` |
| journal/store.rs:1184-1316 | `operation_sequences`, `unresolved_preparation`, `recovery_base{,s}`, `commit_compaction_shards` (sync fn) | `with_connection` | `operation_id` where applicable | compaction outputs | `journal_gate` |
| journal/store.rs:1482-1754 (8) | `committed_segments`, `segment_lengths`, `replay_descriptors`, `sequence_committed`, `latest_sequence`, `committed_segments_for/_after/_through` (sync fn) | `with_connection` | none | none | read paths; `latest_sequence`/`replay_descriptors` are reached without the gate from room/mod.rs:2125 |
| journal/runtime.rs:47, 63 | `MaintenanceBorrow::release`, `::drop` (sync) | `release_maintenance` | job id | releases the maintenance reservation | `journal_gate`; the `Drop` may run without it |
| journal/runtime.rs:152-171 | `acquire_reader`, `renew_reader`, `release_reader` (sync fn) | `acquire/renew/release_journal_reader` | `reader_id` | **the journal reader lease** | none |
| journal/runtime.rs:715, 721 | `JournalRuntime::compact` (async) | `slug_by_storage_id`, `reserve_maintenance` | `maintenance-{operation_id}` | **takes the maintenance reservation** | `journal_gate` |

### storage/maintenance.rs — 35 sites

| File | Function (async?) | Catalogue method(s) | Receipt/operation id | Reservation ownership | Lock/gate held |
| --- | --- | --- | --- | --- | --- |
| maintenance.rs:97-124 (5) | `run_erasure_pass` (sync free fn) | `erasing_accounts`, `erasure_progress`, `erase_account_batch`, `finish_erasure`, `erasure_batch` | the erasure cursor/stage is the durable resume token | none | none |
| maintenance.rs:199, 254 | `enqueue_deletion`, `DeletionWorker::due` (sync) | `with_connection` | none | queues a durable delete job | none |
| maintenance.rs:312-510 (10) | `DeletionWorker::run_once` (async) | `with_connection` ×6, `complete_delete_object`, `document` ×2, `finish_delete` | none | drains pending-delete reservations; `finish_delete` releases capacity | none, except a one-statement `journal_gate` block at :501; `finish_delete` at :510 is outside it |
| maintenance.rs:541, 999 | `enqueue_journal_retirement`, `defer_failed_retirement` (sync) | `with_connection` | none | queues a shared-segment retirement | caller-dependent / `journal_gate` |
| maintenance.rs:595-896 (10) | `rewrite_shared_segment` (async) | `with_connection` ×6, `reserve_object_change` ×2, `commit_object_change` ×2 | synthetic `operation_id` per rewrite | **object reservation for the replacement segment** before the pointer moves | `journal_gate` |
| maintenance.rs:1016-1199 (6) | `JournalRetirementWorker::run_once` (async) | `prune_journal_readers`, `with_connection` ×4, `release_object_accounting_key` | none | releases accounting for reclaimed journal objects | `journal_gate` |

### seed/ and cli/ — 10 sites

| File | Function (async?) | Catalogue method(s) | Receipt/operation id | Reservation ownership | Lock/gate held |
| --- | --- | --- | --- | --- | --- |
| seed/mod.rs:148-196 | `seed_with_backup` (async) | `totals`, `with_connection`, `journal_state`, `set_link_sealing_key` | none | none | deployment `writer.lock` |
| seed/mod.rs:224 | `reset_catalog` (sync fn) | `with_connection` (destructive batch, IMMEDIATE) | none | none | writer lock |
| seed/mod.rs:361 | `seed_with_store` (async) | `journal_state` | none | none | writer lock |
| cli/mod.rs:541-577 (4) | `Command::RotateLinkKey` (async, straight-line sync body) | `link_keyring_primary_id`, `set_link_sealing_key`, `add_link_decryption_key` (loop), `rotate_link_sealing_key` | resume inferred from the durable primary key id | none | deployment `writer.lock` |

`local/`, `storage/blob.rs`, `storage/s3.rs`, `auth/` and `http.rs` have no
call sites. `storage/backup.rs` has none either, but not because it avoids the
database: it opens `catalog.db` with a raw `rusqlite::Connection::open`
(backup.rs:1049, 1197, 1225, 1272) and runs its own integrity and reference
checks, bypassing `Catalog` entirely. No change to `Catalog`'s API surface
reaches it.

### Direct `with_connection` users — 46

| Module | Count |
| --- | --- |
| storage/journal/store.rs | 22 |
| storage/maintenance.rs | 20 |
| seed/mod.rs | 2 |
| document/store.rs | 1 |

These are the ad-hoc SQL surface the catalogue API does not model, and the
migration's main cost. Two are sharp hazards. `document/store.rs:2307` calls
`catalog.open_link_key(..)` from inside a `with_connection` closure, so a
catalogue method re-enters while the connection mutex is held; submitting that
outer closure as a job would deadlock unless the inner call is hoisted out
first. `storage/maintenance.rs:501` takes `journal_gate` for exactly one
statement while `finish_delete` at :510 runs outside it.

### Migration hazards to carry forward

- Catalogue calls made while a room `tokio::Mutex` guard is held — the ones
  that park a runtime worker on `Catalog.connection` — are room/mod.rs:906,
  1972, 1990, 2094; all seven in room/comments.rs; room/suggestions.rs:598;
  room/checkpoint.rs:1448; room/catalog.rs:172-225 via `Room::save`; and
  room/catalog.rs:414/419 via room/figures.rs:522. These must move under
  track 2's lock scopes, not merely behind this boundary.
- Three synchronous `Drop` impls call the catalogue on unwind paths with no
  gate guaranteed: room/mod.rs:395 (`finish_room_write`),
  room/checkpoint.rs:50 (`refund_checkpoint_token`), and
  journal/runtime.rs:63 (`release_maintenance`). An async API has no place to
  await in a `Drop`; these are the first users of the completion hook, which
  is why it takes the connection.
- Only four clusters have genuine idempotency receipts: publication
  (`prepare/commit/abort_operation` with `request_id` + `request_digest`),
  suggestion accept (`request_id` + `acceptance_digest`), comment/reply insert
  (`request_id` + `request_digest`), and journal operations (`operation_id`).
  Accounts, sharing, transfers, labels, renderings and erasure carry no
  operation id, so each needs an idempotent conditional reconciliation rule
  before it is migrated.
- Unbounded reads still on production paths, which need finite result bounds
  or pagination before they become queued jobs with declared byte estimates:
  document/store.rs:1756 and :1826 (`documents()`, full table),
  document/store.rs:2283 (`catalog_entries`), room/catalog.rs:273
  (`load_catalog_checkpoint_rows`, paged but unbounded in total).

## Implementation evidence

Before: every catalogue operation ran synchronously on whichever thread called
it, so 228 production call sites — most of them in asynchronous tasks, 20 of
them while holding a room `tokio::Mutex` — could park a Tokio worker on the
connection mutex or a SQLite busy wait. After: `storage/catalog/execution.rs`
adds a bounded asynchronous entry point beside the unchanged synchronous API,
sharing one connection and one TEMP reservation table. No production caller
has been migrated in this delivery; the boundary is machinery plus the map
above.

Tests, all in `storage/catalog/execution.rs`:

- `blocked_sql_does_not_stop_the_runtime`: a job parked on a channel inside
  SQL, while a Tokio task completes 100 ticks and only then releases it. The
  assertion is on the tick count, not on timing.
- `saturation_is_typed_and_budgets_stay_finite`: `TooLarge` above the request
  limit; all 64 request permits held in `Preparing`; a large request refused
  with `Saturated` rather than parking; a small one waiting and counted in
  `waiting_producers`; and `queued`, `queued_bytes`, `executing` finite
  throughout and back to zero after release.
- `cancellation_before_dispatch_releases_permits_and_runs_no_sql`: a dropped
  reservation, and a submit future aborted while queued behind a full
  executing budget, whose job body is `unreachable!`.
- `cancellation_during_sql_neither_interrupts_nor_rolls_back`: the caller is
  aborted while the transaction is open; the insert still commits, the hook
  still sees `Committed`, and the counters record it.
- `cancellation_before_completion_still_settles_the_hook`: the caller is
  aborted after commit and before the hook; the hook still runs and performs
  its reconciliation SQL.
- `shutdown_settles_executing_work_and_rejects_the_queue`: with one request
  executing and one queued, shutdown does not finish while the transaction
  holds the connection, the queued request gets `ShuttingDown` with a
  `Rejected` hook, the executing one commits, and afterwards both admission
  and the synchronous API report closure.
- `a_job_panic_leaves_the_connection_and_reservations_usable`: a live room
  edit reservation survives a panicking job and a panicking transaction (which
  rolls back), and is readable afterwards both synchronously and through a
  later job.
- `a_failed_transaction_reports_its_error_to_the_hook`: `Failed` reaches the
  hook and the `failed` counter.

Measurements: the budgets above are the declared bounds, and
`CatalogExecutionSnapshot` reports queue wait and execution duration totals
and maxima separately. No throughput measurement is recorded because no
production caller uses the boundary yet; the useful measurement is the
before/after on a migrated caller group, which belongs to the next delivery.

### Room caller migration

Before: all 79 room call sites ran synchronous SQL on the Tokio worker that
reached them, 20 of them while holding a room `tokio::Mutex`. After: 75 of
them are admitted jobs on blocking threads, and the four listed above remain
synchronous with their reasons. No lock was reordered or shortened.

Tests, in `crates/komodoc/src/tests/catalogue_room.rs`, all against a real
catalogue-backed room whose publication receipt was completed the way
production completes it:

- `an_edit_cancelled_before_dispatch_reserves_nothing`: the executing budget
  is filled by two parked jobs, the editor's reservation is observed in
  `queued`, and the task is aborted. Afterwards there is no reservation row,
  the document still reads `first`, and `queued_bytes` is zero.
- `an_edit_cancelled_during_sql_is_settled_by_the_completion_hook`: the
  connection is held, the editor's job is observed `executing`, and the task
  is aborted while its transaction is in flight. The transaction still
  commits, the hook releases the reservation, the update is not applied, and
  the request is counted as completed although nobody was waiting. Removing
  the hook's body fails this test with the reservation still charged.
- `an_edit_cancelled_before_room_completion_releases_its_reservation`: a
  test-only gate keyed by slug parks the caller in the window between its
  reservation committing and the room taking ownership — the window the hook
  cannot observe. Aborting there releases the reservation through `Drop`, the
  update is not applied, and a peer joining afterwards is shown exactly the
  state every other peer has. Removing the `Drop` body fails this test.
- `an_applied_edit_keeps_its_reservation`: the ordinary path still charges and
  keeps the pending snapshot, so the cleanup above is not reachable from it.
- `a_checkpoint_cancelled_during_admission_refunds_its_budget`: the
  publication path's gate parks a checkpoint after its budget bucket is
  charged; aborting there refunds the exact bucket, and the next checkpoint of
  a document with one admission left still runs.
- `a_blocked_catalogue_job_stops_neither_the_runtime_nor_a_cached_room`: with
  SQL parked on a blocking thread, a Tokio task completes 100 ticks and a
  cached room is still retrievable. The assertion is on the tick count, not on
  timing.

Measurements. Byte estimates are declared per request from the owned inputs
themselves: a descriptor request costs `slug.len() + 128`, well under
`SMALL_REQUEST_BYTES`, so an ordinary edit's reservation may wait for
admission rather than being shed; a comment or manifest job is sized from the
rows it carries and capped at `MAX_REQUEST_BYTES`. Job counts per operation
are unchanged or lower: the room's comment load is now one job rather than
`1 + n` connection acquisitions, the publication-staging trio in
`checkpoint_impl_locked` is one job rather than three, the shed-checkpoint
deletions are one job rather than one per row, the rendering retirement pass
is one job rather than one per rendering, and recording plus finishing a
suggestion acceptance is one job rather than two.

Limitations. Cancellation, receipts and peer convergence are now asserted for
the room caller group, but waiting SQL still retains room state on the paths
the gate map above lists, because shortening those scopes is track 2's work
and this delivery deliberately reordered nothing. The caller groups outside
`room/` are unmigrated, so synchronous catalogue calls remain on their request
paths. `storage/backup.rs` opens its own connection and is outside this
boundary by construction. `Catalog::shutdown` is implemented and tested but is
still not wired into the server's shutdown path.
