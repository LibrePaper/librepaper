# User quota and retention preferences

Status: proposed

Related specifications:

- `SPEC-01-history-retention.md` defines source-history storage, physical-byte
  accounting, retention order, and garbage collection.
- `SPEC-02-diff-display.md` defines historical comparison and does not retain
  derived comparisons as durable history.

## Purpose

Give account owners understandable control over how their storage allocation is
used without weakening deployment limits or exposing storage-engine details.
Users may choose a soft history budget, a retention profile, a display timezone, and
milestone preferences. Operators continue to define hard quotas and safety
constraints.

## Goals

- Make current storage use and quota pressure visible before data is thinned.
- Let users trade dense recovery history for lower storage use.
- Distinguish preferences from guarantees and hard deployment limits.
- Preview the consequences of a preference change before it removes history.
- Keep retention deterministic and consistent with physical retained-byte
  accounting.
- Provide useful defaults without requiring users to understand chunking,
  compression, reference counts, or garbage collection.
- Permit account-wide defaults and limited document-specific overrides.

## Non-goals

- Let users raise or bypass an operator-defined hard quota.
- Guarantee indefinite retention of any historical version.
- Expose content-defined chunk sizes, compression formats, object-store layout,
  or garbage-collection scheduling as user preferences.
- Promise that deleting a checkpoint reclaims its logical tree size.
- Charge derived HTML projections, computed diffs, or regenerable caches as
  durable history.
- Replace administrative quota configuration, billing, or plan management.

## Terms

- **Hard quota:** The maximum durable storage an owner may retain, configured by
  the deployment or account plan.
- **Soft history budget:** A user-selected target for storage attributable to
  retained history. Crossing it permits routine history thinning but does not
  reject live edits by itself.
- **Retention profile:** A named or custom policy describing the density of
  routine checkpoints by age.
- **Protected checkpoint:** A named or meaningful milestone retained ahead of
  routine checkpoints, subject to hard count and byte limits.
- **Routine checkpoint:** An event without an enabled protection condition under
  the reason/reference rules in `SPEC-01-history-retention.md`.
- **Charged bytes:** Durable physical bytes attributed to the owner under the
  retained-object accounting rules in `SPEC-01-history-retention.md`.
- **Reclaimable bytes:** Charged bytes that would become unreachable if a
  proposed set of checkpoints or artifacts were removed.

## Policy boundaries

### Deployment controls

The deployment or account plan controls:

- the hard quota;
- any absolute checkpoint count limit;
- the supported age windows and permitted bucket densities for recent history;
- the maximum permitted retention duration or density;
- which artifact classes may be retained;
- emergency eviction order under hard-quota pressure;
- whether document-specific overrides are available;
- grace-period and notification bounds.

These values are constraints, not initial form defaults that a user can
override. The API returns effective bounds alongside editable preferences so
the interface does not duplicate deployment policy.

### User controls

An account owner may configure:

- a soft history budget as bytes or a percentage of the effective hard quota;
- a retention profile;
- the timezone used to display dates and times, not to select retention buckets;
- preferential protection for eligible milestone classes;
- warning thresholds;
- document-specific overrides where allowed.

Editors who do not own the account may view effective policy and storage status
when authorized, but cannot change owner quota preferences unless explicitly
granted that permission.

### System controls

The system chooses chunking, compression, object encoding, cache eviction,
reference-count maintenance, and garbage-collection implementation. These
choices may change without modifying user preferences.

The initial implementation uses native server-side FastCDC, SHA-256, and zstd
under `SPEC-01-history-retention.md`. Browser/WASM compression is not required.
Worker/queued-byte limits, whole-file fallback, and optional packing/compaction
are deployment implementation controls, not account settings. None may bypass
hard quota or physical disk headroom. Rendering and diffing remain browser work.

## Preference model

Conceptually, the durable preference record is:

```json
{
  "version": 1,
  "historyBudget": { "kind": "percent", "value": 60 },
  "retentionProfile": "balanced",
  "retentionPolicyVersion": 1,
  "customRetention": null,
  "displayTimezone": "America/Toronto",
  "milestonePreferences": {
    "named": true,
    "cli": true,
    "publish": true,
    "restore": true,
    "accept": true,
    "comment": true
  },
  "warningThresholds": [75, 90],
  "documentOverrides": {}
}
```

The concrete schema may differ, but it must be versioned. Store the user's
intent separately from the effective policy after deployment constraints are
applied. This allows a plan or operator limit to change without silently
rewriting the saved preference.

Unknown fields from a newer schema must not cause destructive fallback. A
reader that cannot interpret a preference preserves the last understood
effective policy and reports the incompatibility; it cannot authorize destructive
thinning from an invented fallback. Independently understood hard limits still
apply. The Balanced identifier plus policy version resolves to fixed tier values;
changing defaults does not silently reinterpret an existing saved policy.

### Soft history budget

The initial interface supports either:

- a percentage of the effective hard quota; or
- an absolute byte amount.

The API normalizes both to an effective byte target at evaluation time. A
percentage follows changes to the hard quota; an absolute target does not.

The soft budget applies to checkpoint metadata and durable objects retained only
for history, including recipes, chunks, whole-file objects, and historical assets.
Only the latest published PDF and its companions are retained, outside the soft
history budget but inside the hard quota; superseded bundles are collected
regardless of retention preferences. Bytes also required by the live document are
not reclaimable history merely because an old checkpoint references them.
Shared objects are charged once and attributed consistently with
`SPEC-01-history-retention.md`.

Assign objects also needed by live source/assets to the live category, not again
to soft history usage. Count history-only shared objects once across all their
retained checkpoints. There is no cross-document object sharing to apportion.
Checkpoint count is not a byte estimate: deleting many events can release little
payload if their chunks remain referenced. Stored compressed lengths determine
the owner charge; filesystem slack and maintenance headroom remain separate
deployment measurements and constraints.

Crossing the soft budget:

1. evicts regenerable caches and unprotected derived renderings where relevant;
2. selects routine checkpoints according to the effective retention policy;
3. uses the deterministic cumulative candidate order from SPEC-01: age/count
   rules may remove events without unique payload savings; additional byte-driven
   removal requires positive reclaimable bytes for the selected set, including
   metadata and jointly freed objects, not necessarily each event individually;
4. preserves protected checkpoints and the newest checkpoint.

The soft budget is a target, not a strict cap. Shared live data or protected
history may keep history above it. Live edits must not fail merely because the
soft target was crossed.

### Retention profiles

Ship with three stable user-facing profiles whose precise values are returned
by the server rather than duplicated in clients:

- **More recovery points:** keeps dense routine history longer.
- **Balanced:** uses the default windows from
  `SPEC-01-history-retention.md` unless deployment policy changes them.
- **Use less storage:** thins routine history sooner while respecting the
  deployment minimum.

Balanced policy version 1 uses the storage-conscious tiers below, not an initial
24-hour keep-everything window:

| Checkpoint age | Routine retention density |
| --- | --- |
| Less than 1 hour | Newest per UTC-aligned 5-minute bucket |
| At least 1 hour, less than 24 hours | Newest per UTC clock-hour bucket |
| At least 24 hours, less than 7 days | Newest per UTC-aligned 6-hour bucket |
| At least 7 days | Newest per UTC calendar-day bucket within effective limits |

Describe this as roughly 60 routine recovery points during the first week of
dense activity, plus protected milestones and older daily points. Partial UTC
buckets can raise the first-week routine count to 62; leases/grace periods can
temporarily retain more. Neither description is a fixed storage allowance or a
guarantee of a checkpoint in an empty or already pruned bucket. Hard limits
override all tier preferences. Returning effective tier values and count/byte
bounds lets the interface explain this without maintaining a separate algorithm.

An optional custom profile exposes a bounded list of increasing age upper
bounds and their bucket widths, final-tier expiration or continuation, and an
optional maximum routine-checkpoint count. Widths must be positive supported
UTC-aligned durations, nondecreasing with age; age windows are contiguous and
half-open. Do not provide an unbounded "keep every save" tier or a cron-like
language. Custom and named profiles use the same backend evaluator and tie rules
as SPEC-01, keeping the newest eligible routine event per occupied bucket.
More/less-dense presets must publish their actual versioned values and obey the
deployment bounds; the labels alone do not define retention.

Changing the checkpoint creation cadence is outside this preference. Retention
controls which checkpoints survive, not how often active work is initially
captured.

Ordinary live-save durability is independent of both. Explain: “Your edits are
saved promptly; history keeps selected recovery points.” Do not imply that a
five-minute retention bucket delays saving for five minutes, or that every
successful live save creates a permanently available checkpoint.

### Display timezone

All retention buckets use fixed UTC boundaries, including five-minute and
six-hour buckets. An optional IANA display timezone controls timeline and
preview labels only; UTC is the presentation fallback if it is missing or no
longer recognized. Daylight-saving changes cannot affect retained winners.
The settings UI must identify UTC as the retention boundary reference even when
timestamps are displayed locally. Changing display timezone is non-destructive
and does not require a thinning preview or rewrite history policy.

### Milestone preferences

Users may indicate that named, explicit CLI milestone, publish, restore, and
accepted-suggestion events should receive preferential protection, along with
revisions referenced by open comments/pending suggestions. Defaults match the
protection table in SPEC-01. An event remains protected while any enabled
condition applies; disabling one preference cannot erase another condition.
Disabling a preference makes otherwise unprotected events eligible for routine
retention through the normal preview/application flow, not immediate deletion.

The `comment` preference applies to open annotation references, not every event
whose reason happens to be `comment`. Resolving/deleting the last open reference
releases that preference; the checkpoint becomes routine unless another enabled
condition protects it. Acceptance can create its own protected `accept` event.
Reopening does not recreate an evicted revision; retain quotations and report
history unavailable as required by SPEC-01. This avoids keeping hundreds of
resolved-review checkpoints forever.

The interface must say “preferentially retain,” not “keep forever.” Under the
hard quota, the system may remove protected historical versions only after
regenerable data, unprotected artifacts, and routine candidates are exhausted,
as specified in `SPEC-01-history-retention.md`.

Naming a version must show the same limitation. Names are organizational and
protective metadata, not an unlimited-storage guarantee.

Milestone preferences apply to source history only. Neither naming a version nor
choosing a denser retention profile preserves an older PDF. The latest-only PDF
policy is fixed; these controls do not provide a historical-rendering archive.

### Document overrides

Account preferences apply by default to every owned document. Where enabled, a
document may override the retention profile, soft history budget, and milestone
preferences. It inherits display timezone and warning thresholds from the owner.

A document override cannot raise its effective limits beyond the account or
deployment constraints. The sum of document soft budgets may exceed the account
hard quota because they are targets, but the interface must warn when configured
targets cannot all be satisfied simultaneously.

Account-level hard-quota enforcement considers all documents owned by the
account. It must not exhaustively destroy one document merely because that
document first crossed its soft target when another candidate set reclaims
space with less loss of protected history.

## Storage status and explanation

Show an owner-facing storage breakdown containing at least:

- total charged bytes and hard quota;
- live document state and current source objects;
- source history;
- assets;
- the latest published PDF and its SyncTeX/provenance companions;
- other durable metadata;
- reclaimable unprotected history;
- regenerable cache usage when it is useful, clearly marked as disposable and
  excluded from durable quota where applicable.

The categories must not double-count shared objects. If exact category
attribution is ambiguous, assign each object to one documented primary category
and expose the total as authoritative.

Display both logical history size and charged physical size only when the labels
make their distinction clear. The quota meter uses charged physical bytes.

Do not describe a checkpoint's logical tree size as reclaimable. For a manual
or simulated removal, report the bytes expected to become unreachable after
considering every retained checkpoint and the live document.

## Preview and application

Saving a preference that broadens retention is non-destructive and may take
effect immediately. Saving a preference that makes retained checkpoints newly
eligible for removal follows two phases:

1. **Preview:** Calculate the effective policy, affected checkpoint counts by
   class, estimated reclaimable bytes, oldest affected date, and any protected
   checkpoints that would become unprotected.
2. **Apply:** Persist the preference and enqueue deterministic thinning using a
   policy generation identifier.

The preview must identify estimates as estimates when concurrent edits,
deduplication, or delayed garbage collection can change the final result. It
must never claim that shared logical bytes will be reclaimed.

For material reductions, the interface requires explicit confirmation and
offers a deployment-bounded grace period. Mark affected checkpoints as pending
removal but keep them visible/restorable until that period ends. Their bytes
remain charged until physical reclamation; grace retention is bounded, not a
hidden second history archive. Hard count/quota enforcement may shorten or bypass
the grace period, but never break active read/restore leases or claim future
GC savings as free space. Ordinary ongoing thinning under an already accepted
profile does not require a new confirmation for every bucket replacement.

An apply request includes the preference revision and preview generation. If
the policy, quota, or relevant retained-object catalogue changed enough to make
the preview materially inaccurate, reject the apply request and require a new
preview.

## Hard-quota behavior

User preferences never change the hard-quota eviction order. When charged
storage reaches the hard quota:

1. apply the eviction and thinning order in `SPEC-01-history-retention.md`;
2. use user retention and milestone preferences to order candidates within the
   classes where the deployment permits choice;
3. retain the newest checkpoint and live session;
4. plan reclaimable sets before deleting history for a proposed write, and
   reject/defer infeasible growth rather than deleting protected history in a
   futile attempt to make it fit. Only successful GC releases reserved capacity.

The UI distinguishes:

- below the soft target;
- above the soft target and eligible for routine thinning;
- near the hard quota;
- at the hard quota with writes or artifact creation restricted.

It must explain what will happen next rather than presenting every state as a
generic “storage full” error.

An encoding queue delay is a different condition from quota exhaustion. Show
live-save durability and pending historical-checkpoint status separately; no
preference may bypass the server's bounded worker/admission limits. A whole-file
fallback remains charged and cannot quietly accumulate outside the storage meter.

## Notifications

Owners may choose warning thresholds within deployment bounds. Notifications
are based on the percentage of the hard quota consumed, not the percentage of
the soft history budget consumed. The interface separately reports when routine
thinning begins because the soft target was exceeded.

Notifications should cover:

- crossing a warning threshold;
- automatic routine thinning starting and completing;
- protected history being at risk under the hard quota;
- writes or artifact creation being restricted;
- a repair changing the charged-byte total materially.

Coalesce repeated notifications and avoid including source names or content in
channels where that metadata is not authorized.

## API requirements

The quota-preference API must provide:

- saved preferences and their revision;
- effective preferences after applying deployment constraints;
- the source and explanation of each constraint;
- current charged bytes, hard quota, soft history target, and category totals;
- a policy preview endpoint;
- an idempotent apply operation tied to a preview and preference revision;
- thinning status and last completed policy generation.

All mutations require owner-level authorization or an explicit quota-management
permission. Reads must respect document and account authorization boundaries.
Audit preference changes, confirmations, automatic thinning, and administrative
overrides without recording document content.

## Concurrency and failure recovery

Preference updates use optimistic concurrency. A stale client cannot overwrite
a newer preference record silently.

Thinning uses a durable policy generation and is idempotent. A crash between
catalogue removal and object garbage collection must leave all retained
checkpoints readable. Garbage collection runs only after the retained manifest
and reference metadata are durable.

Concurrent checkpoint creation may increase storage during a preview or
thinning run. The newest checkpoint remains protected, and the system
reevaluates the effective policy after the current transaction rather than
deleting an object based on stale reachability data.

Quota-repair jobs may correct charged-byte totals. They do not silently change
saved preferences, but may trigger a new effective-policy evaluation and owner
notification.

## Accessibility

- Storage meters expose numeric values and status text, not color alone.
- Policy presets and custom fields are keyboard accessible and have explicit
  descriptions of their retention consequences.
- Preview results announce affected counts, dates, and estimated reclaimed
  bytes in a logical reading order.
- Confirmation does not rely on a transient toast.
- Warning and hard-quota states use distinct programmatic labels.

## Observability

Record aggregate measurements without document content:

- preference and retention-profile adoption;
- soft-target crossings;
- previewed, confirmed, cancelled, and completed thinning operations;
- estimated versus actually reclaimed bytes;
- checkpoints removed by class and policy generation;
- hard-quota interventions and restricted writes;
- grace-period restorations;
- reference-accounting repair discrepancies.

Emit structured server logs and audit/benchmark reports initially; a new metrics
endpoint is not required. These measurements must not expose document titles,
paths, source text, or private milestone names.

## Migration

1. Complete retained physical-byte accounting and repair support required by
   `SPEC-01-history-retention.md`.
2. Introduce versioned account preferences with the balanced profile as the
   default for new histories, without silently changing existing retention.
   Existing histories require an explicit preview/apply migration to the tighter
   profile and its bounded grace process (or equivalent operator approval).
3. Expose read-only storage breakdowns and effective deployment constraints.
4. Add policy preview and audit its estimated reclaimable bytes against actual
   garbage collection.
5. Enable preference changes with confirmation and a grace period.
6. Add document overrides only after account-wide enforcement and explanation
   are reliable.

Do not expose a misleading logical-size quota meter as an interim user-facing
implementation. Until physical retained-byte accounting is authoritative,
preferences may be saved and previews may be marked experimental, but they
must not claim precise reclamation or replace existing enforcement.

Age/count-tier selection can ship independently as described in SPEC-01; only
physical-byte budget enforcement and precise byte previews wait for verified
accounting. Both rollouts must share effective policy versions and the destructive
migration safeguards rather than silently applying conflicting defaults.

## Acceptance criteria

1. A user can select a soft history budget, retention profile, and IANA display
   timezone without changing deployment hard quotas or UTC retention boundaries.
2. The API returns saved and effective preferences and explains every operator
   constraint that changes the effective result.
3. Balanced retention matches `SPEC-01-history-retention.md` unless bounded by
   explicit deployment policy.
4. Custom retention is deterministic, uses bounded UTC-aligned tiers, and selects
   the newest eligible routine event per occupied bucket. Display timezone and
   daylight-saving changes cannot change selection or require destructive apply.
5. Crossing the soft history target thins eligible routine history but does not
   reject live edits or remove protected checkpoints by itself.
6. Hard-quota pressure follows the global eviction order regardless of user
   preferences and never removes the newest checkpoint or live session.
7. Storage totals count durable shared objects once and never present logical
   tree bytes as physically reclaimable bytes.
8. A destructive preference change shows affected checkpoint counts, dates,
   protection changes, and estimated reclaimable bytes before application.
9. Applying a stale or materially invalid preview is rejected safely.
10. Preference application and thinning are idempotent and recover safely from
    crashes before garbage collection.
11. Named and milestone checkpoints are described and treated as preferential,
    not guaranteed, retention.
12. Account and document settings cannot exceed plan or deployment constraints.
13. Unauthorized editors cannot modify quota preferences or learn storage data
    outside their authorization scope.
14. Warning, thinning, and hard-quota states are visually and programmatically
    distinct.
15. Focused tests cover shared-object accounting, zero-reclaim checkpoint
    removal, UTC tier/bucket boundaries and display timezone changes, concurrent checkpoint
    creation, stale previews, grace-period restoration, operator limit changes,
    and mixed account/document policies.
16. Balanced version 1 matches the five-minute/hourly/six-hourly/daily rules and
    exact age boundaries in SPEC-01. No keep-everything recent tier is introduced
    by an unknown schema, a preset label, or a default-policy upgrade.
17. Open-reference protection ends when the last applicable annotation resolves;
    disabling one milestone preference leaves other enabled protection intact.
    Neither protected history nor preferences archive older PDFs.
18. New checkpoint publication replaces routine bucket winners without delaying
    ordinary live saving; the UI distinguishes saved work, pending checkpoint
    admission, and retained history. Fallback objects and grace retention remain
    accounted and bounded until successful reclamation.

## Open questions

- What reductions are material enough to require explicit confirmation or a
  grace period?
- Which bounded, versioned tier values should the optional More recovery points
  and Use less storage presets expose alongside Balanced?
- Which storage warnings belong in-product, by email, or in operator-managed
  notification channels?
