# 8. Small shared policies

Status: proposed independent changes. Inherits the full compatibility table in
[umbrella section 8](../../../SPEC-refactor.md#8-consolidate-small-shared-policies-and-helpers).

## Scope and delivery

Use one reviewable change per policy below. Before extracting, inventory callers
and record differences that must remain explicit. Do not bundle these changes
with the catalogue or room concurrency migrations.

| Change | Required evidence |
| --- | --- |
| Checkpoint tree identity accessor | Legacy `sha` fallback and row conversion preserve distinct event identities. |
| Default main-file policy | Explicit names, format defaults, and unknown fallback remain unchanged. |
| Comment view conversion | Snapshot/event privacy and viewer-specific flags agree for author, owner, and other viewers. |
| Rendering publication helper | Reservation/upload/registration/cleanup stay consistent; current and historical validation retain their separate final checks. |
| UTF-16 helpers | Inventory room/session/CLI/text helpers; specify checked/clamped behavior, surrogate boundaries, overflow, and invalid ranges. |
| Request hashing | Shared byte hashing preserves receipts; canonicalization changes require byte-equivalence evidence or a versioned migration. |
| Bulk comment reconciliation | Identify remaining legacy/import/fixture/production callers; batch production queries if retained. |
| Unused fields/documentation | Verify `Peer.address` and `attach_deployment_lock_unavailable` across supported configurations before removal. |

## Acceptance

Retain focused regressions and test policy boundaries, including non-BMP and
combining characters for UTF-16, and nested keys, escapes, and numbers for request
digests. Do not reimplement already completed cleanup or write tests that merely
repeat the new helper's implementation. Rendering helper extraction must pass
the existing cancellation, authority, and physical-reclamation regressions.
