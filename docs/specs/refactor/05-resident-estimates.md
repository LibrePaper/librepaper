# 5. Resident memory estimates

Status: implemented; profiling gate passed. Inherits
[umbrella section 5](../../../SPEC-refactor.md#5-complete-resident-size-caching).

## Decision gate and scope

Profile admission after track 2. Proceed if repeated comment/manifest
serialization remains a material cost; record the workload and result here.
Existing generation-based CRDT size caching is already implemented.

Cache component estimates for comments/replies, manifest metadata, assets,
renderings, and source state with an explicit mutation-to-invalidation inventory.
Document included memory and preserve conservative saturating arithmetic and
failed-estimate behavior. These are admission estimates, not allocator readings
or storage quota charges.

Do not introduce a running registry total in this delivery. That optimization
requires a separate accounting protocol covering update, replacement, admission,
and eviction without reversing lock order or transiently undercounting memory.

## Acceptance

- Unchanged admission scans do no CRDT encoding or comment/manifest serialization.
- Exercise each mutation category, restore, failure, and invalidation boundary.
- Existing byte-ceiling and active-owner admission behavior survives, including
  arithmetic overflow and estimate failure.
- Record serialization counts and admission cost before/after; preserve the
  track 2 lock order. If profiling does not justify work, record deferral.

## Implementation evidence

Status: implemented. Profiling justified the work: the gate workload is 1,000
comments with 1 KiB bodies and 64 checkpoints, estimated 200 times without
touching the room in between, and it is kept as the ignored regression
`room::resident::tests::profile_unchanged_resident_metadata` so the number can
be reproduced rather than remembered. Before: 3.84 s, 400 serializations, 0
encodes (the generation-keyed CRDT cache already held). After: 21.5 ms and
23.6 ms on two runs, 2 serializations, 0 encodes -- one pass over each of the
two cached components, then nothing. Before and after report the same
1,242,854-byte estimate, which is what
says the caching changed the cost and not the answer. Debug build, one machine,
same process; the ratio is the measurement, not the absolute time.

`Room::resident_bytes` is now `RoomState::resident_estimate` in
`room/resident.rs`, called with the state lock held and nothing else. The
registry lock order is unchanged: estimates still await room state only outside
both registries, and no running registry total was introduced, so admission and
eviction publish no accounting of their own.

### What the estimate includes

Encoded CRDT state for the exact current generation; the comments list with its
replies as serialized JSON; the retained manifest as serialized JSON; and one
`(String, i64)` per resident asset and per resident rendering, which counts the
bookkeeping maps rather than the objects themselves -- those live in storage and
are charged to the storage quota, never here. It remains an admission measure,
not an allocator reading. Components are added with saturating arithmetic, and a
component that cannot be serialized counts as `usize::MAX`, so an unmeasurable
room is refused rather than admitted; that failure is cached like any other
measurement and retried after the next mutation.

### Mutation-to-invalidation inventory

Comments and the manifest are held in `resident::Measured<T>`, a wrapper whose
`Deref` reads freely and whose `DerefMut` drops the cached measurement. The
inventory is therefore enforced by the borrow checker rather than by a list of
`invalidate()` calls that a later change could forget to extend:

| Component | Mutations | How it is invalidated |
| --- | --- | --- |
| Comments and replies | add, delete, resolve, reply, anchor, suggestion accept and reject, catalogue load, catalogue save renumbering, erasure scrub | every write path takes `&mut state.comments`, which clears the cached size |
| Manifest metadata | checkpoint, label, restore, prune, manifest reload, erasure attribution scrub | every write path takes `&mut state.manifest` or assigns through it |
| Source state | edits, restores, main-file and asset naming | `session.generation` moves, and the estimate re-encodes for the new generation exactly once |
| Assets | upload, naming, prune | counted live from `asset_sizes` cardinality, so there is no cached value to go stale |
| Renderings | publish, retire, prune | counted live from `rendering_sizes` cardinality |

Assets and renderings are deliberately not cached. Their component is a map
length multiplied by a constant, which costs nothing to recompute; caching it
would add state whose only possible behaviour is to be wrong.

### Tests

`room::resident::tests` covers: unchanged scans doing zero encodes and zero
serializations, both directly and through `RoomSet::cached_bytes` and
`evict_idle`, which are the admission and eviction scans; each comment mutation
category through `apply_command`; suggestion acceptance and rejection;
checkpoint, label, restore and manifest pruning; asset upload, naming and
rendering publication; a source edit re-encoding once and only once; the erasure
scrub, which mutates resident rows from the registry rather than through a room
API; and the failure boundary, where an unserializable component measures as
`usize::MAX`, caches that answer, saturates the total, and is measured again
after a mutation. Every one of those assertions also compares the cached
estimate against a from-scratch recomputation, so a stale component fails the
test even when its serialization count looks right. Counting uses thread-local
counters so a test sees only its own work while the suite runs in parallel.

The existing byte-ceiling and active-owner admission tests
(`tests::room_lifecycle_fixes`) pass unchanged, which is what says the estimate
still returns the same numbers to the same decisions.

### Limitations

The estimate still measures serialized JSON length rather than resident
allocation, so it under-reports the heap a `Vec<Comment>` actually occupies; it
is a budget unit, not a measurement, and nothing here changed that. A `&mut`
borrow that does not mutate invalidates anyway, which costs one re-measure --
`save` takes one on every flush, for example. A running registry total is still
future work and still requires the accounting protocol this contract describes.
