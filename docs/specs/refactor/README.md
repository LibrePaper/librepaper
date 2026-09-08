# Refactor implementation specs

Status: proposed; these documents authorize no claim of implemented behavior.

These are the implementation contracts for [SPEC-refactor.md](../../../SPEC-refactor.md).
All tracks inherit its durability, authorization, compatibility, accounting,
and validation requirements. Section numbers match the umbrella, not delivery order.
The umbrella defines shared guarantees; these files define delivery boundaries.
Update both if a decision changes their common requirements.

## Decision standard

Evaluate each choice from the failure or cost it must address. State the
invariant, identify the smallest change that enforces it, and compare added
state, memory, migration cost, and failure modes. An architecture named in a
spec is a candidate unless its tradeoff has been justified. Measurements select
performance optimizations; correctness requirements do not wait for profiling.

The first size-limit delivery narrows acceptance because admitting an update
that cannot survive persistence violates the acknowledgement contract. Stable
identity is early because mutable display names cannot establish historical
account identity. Off-thread SQL is justified by blocking operations on async
workers; a particular worker architecture still needs a cost comparison.

Numeric memory defaults, the complete lock-order inventory, and remote backup
ownership acquisition remain design work. These documents are bounded starting
contracts, not claims that those decisions have already been validated.

## Delivery order

| Milestone | Contract | Dependency and completion boundary |
| --- | --- | --- |
| First | [10. Size limits](10-size-limits.md) | Independent correctness fix; reject unsupported work before acceptance. |
| Early | [9. Stable attribution](09-attribution.md) | Independent correctness fix; schema, writers, erasure, and backup compatibility ship together. |
| Foundation | [1. Catalogue execution](01-catalogue-execution.md) | Define lifecycle and budgets before caller migration; coordinate with track 2. |
| Foundation | [2. Lock scopes](02-lock-scopes.md) | Registry work can land independently; catalogue callers follow the track 1 contract. |
| Performance | [3. Shared retention](03-retention.md) | Use the established catalogue boundary and lock order. |
| Performance | [4. Rendering lookup](04-rendering-lookup.md) | Use the established catalogue boundary; independent of retention implementation. |
| Profile first | [5. Resident estimates](05-resident-estimates.md) | Measure after registry lock changes; defer unless serialization remains material. |
| API | [6. Write errors](06-write-errors.md) | Narrow error types needed by earlier correctness fixes can land with those fixes. |
| API | [7. Validated commands](07-commands.md) | Coordinate error mapping with track 6; preserve wire compatibility. |
| Independent | [8. Shared helpers](08-helpers.md) | Separate small changes; do not bundle with concurrency work. |
| Operational | [11. S3 operations](11-s3.md) | Preserve per-object accounting and ambiguous-write handling. |
| Before concurrent API exposure | [12. Backup ownership](12-backup-ownership.md) | Keep external serialization until the ownership protocol is implemented. |

Before coding each track, record its affected call sites and any remaining
design decisions in that spec. Keep schema migrations and all dependent caller
changes in a coherent delivery. For cross-cutting tracks, use small preparatory
commits without declaring the track complete while unsafe old paths remain.

Every delivery records the before/after behavior, focused validation, and any
remaining limitation. Performance tracks also record operation counts or
measurements on the same workload. Run the umbrella's required checks before
merging; historical test totals are not acceptance thresholds.
