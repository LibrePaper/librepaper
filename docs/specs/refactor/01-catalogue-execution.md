# 1. Catalogue execution and cancellation

Status: proposed foundation contract. Inherits
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
