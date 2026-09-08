# 2. Room and registry lock scopes

Status: proposed. Inherits [umbrella section 2](../../../SPEC-refactor.md#2-shorten-remaining-room-and-registry-lock-scopes).

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

The legacy-comment and catalogue wait portions, and the complete room operation
gate audit, remain outstanding. Registry work alone does not complete this track.
