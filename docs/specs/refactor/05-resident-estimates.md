# 5. Resident memory estimates

Status: proposed, conditional on profiling. Inherits
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
