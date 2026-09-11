# Source history storage and retention

Status: proposed

Dependencies: byte-driven retention requires verified retained-object accounting.
Age/count retention and `SPEC-02-diff-display.md` can ship independently of the
new encoding, provided logical tree identities and snapshot reads remain stable.

Rendered comparison and tracked-change presentation are specified in
`SPEC-02-diff-display.md`.

## Purpose

Store exact historical source efficiently, charge its retained physical bytes
to the owner, and thin routine checkpoints predictably under count or byte
limits. Checkpoints remain complete logical project trees while successive text
versions reuse compressed content-defined chunks.

## Goals

- Preserve exact historical source trees while storing successive text
  revisions approximately in proportion to what changed.
- Charge source history according to the durable storage it retains.
- Bound storage growth from frequent checkpoints.
- Preserve named versions and meaningful milestones preferentially.
- Keep each retained checkpoint directly readable and restorable.
- Bound server encoding work independently of the number of connected editors.

## Non-goals

- Store source history as chains of patches.
- Define rendered diff presentation or history-panel interaction.
- Preserve disposable HTML previews or computed diffs as history.
- Ship browser/WASM history encoding or a client-prepared chunk upload protocol.
- Build a general-purpose compression/chunker tuning framework.

## Existing behavior

The server records a checkpoint as a complete content-addressed tree. The tree
names every path and content digest; unchanged file and asset blobs are reused
between checkpoints. The `parent` and `changed` fields describe chronology and
changed paths, but reconstruction does not replay them.

Physical source storage is content-addressed, but history quota accounting is
logical: it currently sums the full tree size recorded on every retained
checkpoint. A file reused by twenty checkpoints is stored once but charged
twenty times.

Existing retention preferentially protects non-empty labels, not checkpoint
reasons. Garbage collection and rolling-hour checkpoint admission budgets
already exist. Legacy pre-tree checkpoints and `sources/<slug>` objects remain
readable compatibility formats. This proposal extends those mechanisms.

## Design

### Source history remains authoritative

Keep each checkpoint as a complete tree manifest. Change the storage beneath a
text entry so that a new version need not add another complete file blob.
Checkpoint APIs continue to return complete files and complete trees; chunking
is an internal storage encoding.

Logical tree serialization and `tree_sha` must not include encoding, recipe
digests, compression parameters, or stored lengths. Preserve existing file
identities and the tree's source-affecting compile settings (`engine`, `release`).
A versioned storage catalogue maps `(document storage identity, file digest)`
to a whole-file object or recipe. Re-encoding identical source must not change
the tree digest, checkpoint identity, labels, or rendering associations.

This property is required for:

- direct access to any retained version;
- restoration without replaying an edit chain;
- safe removal of intermediate checkpoints;
- independent verification by tree and blob digest;
- complete multi-file snapshots in which included chapters, bibliographies,
  settings, and assets cannot come from different moments.

The history service may retain parent links and changed-path summaries for
presentation. Neither is required to reconstruct a checkpoint.

### Chunked text history

Store historical text files as content-defined chunks rather than one object
per complete file version. A text entry retains the digest and uncompressed
size of the complete file; the storage catalogue resolves it to an ordered chunk
manifest. Each chunk is content-addressed within the document's storage identity
and compressed independently. Use the established Rust `fastcdc` crate's
`v2020` implementation, not a custom rolling-hash implementation. Use zstd as
the one new compression format, initially at level 3. Retain SHA-256 for logical
file/tree identity and new chunk/recipe integrity; do not introduce BLAKE3 or a
non-cryptographic object identity in this rollout. FastCDC's boundary fingerprint
is not a content-integrity hash.

Content-defined boundaries are required. Fixed-size chunks cause an insertion
near the beginning of a file to shift every later boundary and rewrite nearly
the whole file. FastCDC keeps boundaries stable after local
insertions and deletions, allowing unchanged regions to reuse existing chunks.

A representative encoding is:

```text
checkpoint tree
  paper.tex -> file digest F2

storage catalogue (outside the logical tree)
  F2 -> recipe R2

recipe R2
  [chunk A, chunk B2, chunk C, chunk D]

previous recipe R1
  [chunk A, chunk B1, chunk C, chunk D]
```

Only `B2`, the complete recipe, the checkpoint tree, and their catalogue/event
metadata are new history storage. The owner is charged for those newly retained
bytes. Removing the last reference to `B1` makes it reclaimable; successful
garbage collection then reduces the owner's charged history.

Use conservative minimum, target, and maximum chunk sizes so ordinary prose
edits usually replace a small region without excessive object and recipe
overhead; boundary resynchronization is not a guarantee of a fixed-size change.
The native benchmark used `fastcdc 5.0.0`, normalization Level1, seed 0, targets
of 1 and 4 KiB, minimum target/4, and maximum target*8. These are measured
candidates, not final production parameters. Select one initial profile and a
small-file threshold with a bounded benchmark of representative retained
histories, not per-document algorithm tuning. Pin the crate/profile and record
boundary fixtures; dependency upgrades must not silently change writer behavior.

Use a compact, versioned binary recipe with raw 32-byte digests, ordered chunk
references, raw lengths, and explicit codec/format identity. Avoid repeated hex
digests and verbose per-chunk JSON in durable recipes. A recipe is complete for
its file, so its size still grows with the number of chunks even for a one-word
edit. The benchmark's 48-byte header and 36-byte references are an example, not
a frozen ABI. Finalize the decoding format before enabling writes.

Readers reconstruct from explicit references without running FastCDC. Changing
writer boundary parameters requires an explicit encoding-profile revision but
does not itself require a new recipe decoding version; changing the wire
structure or codec contract does. Old recipes remain readable in either case.

File identity remains the digest of canonical uncompressed bytes. On read,
decompress each named chunk and concatenate in recipe order, verify the size and
digest, and only then return the file. Chunk and recipe digests are additional
storage-integrity checks rather than replacements for complete-file identity.

Below the measured small-text threshold, use one whole-file zstd object; large
text uses the chunked encoding. This keeps complexity where reuse pays off.
Assets and binary files remain whole content-addressed objects. Preserve their
existing encoding; do not routinely recompress already compressed PDFs/images.
Whole-file compression is also the bounded fallback described below. Threshold
selection must include recipe/object overhead and savings after retention, not
just raw file size or compressed payload ratios.

This design deliberately avoids a parent-dependent patch chain. Any retained
file version can be reconstructed directly from its recipe and chunks, so
removing an intermediate checkpoint requires reference-count updates and
garbage collection but no rebasing or promotion of later deltas.

### Execution placement and bounded server work

The server performs the entire history encoding pipeline: capture authoritative
source, compute full-file SHA-256, find FastCDC boundaries, hash chunks, look up
existing objects, compress only missing chunks, and serialize recipes. Storage
publication, accounting, retention, GC, and verified reconstruction also remain
server-side. Browser clients continue using complete-file/tree APIs. They do
not download a history compression module or prepare/upload chunk recipes.
Rendering, semantic projection, and visual/source diff computation remain in the
browser under `SPEC-02-diff-display.md`.

Run CPU-heavy encoding/reconstruction in an explicitly bounded native worker
pool, outside async request executors and room-state locks. Bound queued jobs,
queued input bytes, per-job input/chunk counts, and memory as well as worker
count. A generic blocking executor's default limit is not the resource policy.
Capture an immutable input identity under the existing publication/snapshot
rules, then release locks before expensive work. A job cannot publish a recipe
for a different live generation merely because the document changed meanwhile.

Deduplicate in-flight encoding by document storage identity and full-file
digest. Multiple collaborators must not cause duplicate encoding of the same
file. Reuse existing encodings for unchanged file digests, and reuse zstd
contexts within workers. Do not perform per-chunk subprocess launches or
object-store listings. Batch metadata lookups and bound storage-request
concurrency separately. Spread automatic work over time and apply fair
per-account admission so large documents cannot monopolize the pool.

On overload, defer/coalesce uncommitted routine checkpoint requests without
delaying the independent live-session durability path. Preserve explicit
milestone identity and return a pending/retry state when it cannot yet commit;
never silently merge distinct named/restore/publication events. If a checkpoint
must proceed without chunking, permit a whole-file encoding only within the
same bounded work, quota, and temporary-storage budgets. Compression itself is
work; fallback is not permission for an unbounded second queue or disk backlog.
When neither encoding fits, defer/refuse the history growth with an explicit
status, not a false checkpoint-success acknowledgement.

Server compaction may later replace a fallback encoding, or choose whole-file
compression for sparsely retained history, but it is not required for a read or
restore. It must demonstrate a positive net retained-byte saving after shared
references, use bounded maintenance resources and reserved temporary space,
verify the replacement, atomically switch the encoding lookup, and lease/GC the
old representation. Do not keep both indefinitely or assume compaction can
rescue an already full disk. Initial correctness does not depend on compaction.

### Alternatives and deferred optimizations

- Pairwise forward deltas make access to an old version depend on replaying
  every preceding patch after a base snapshot.
- Reverse deltas optimize the newest version but still require chain traversal
  and rewrite work when retention removes an intermediate version.
- Periodic full snapshots bound those chains but introduce compaction and base
  selection without helping unchanged regions across snapshot boundaries.
- Yjs state vectors identify collaborative states but do not provide a compact,
  independently prunable archival representation of deleted source.
- Whole-file compression helps substantially but still stores almost the same
  compressed prose again after a one-word edit. It remains the fallback for
  small files and may be combined with chunking.
- One-hop compression against an immutable base is not a checkpoint replay
  chain. The earlier benchmark showed strong localized-edit savings, so it is
  a credible future alternative, not rejected as inherently unsafe. Defer it
  from the initial codec: base selection, charged dependencies, compaction,
  verification, and backup/GC support add complexity to the chosen direct-chunk
  model. Do not implement several encodings speculatively.
- Per-document zstd dictionaries remain a benchmark candidate, not an initial
  runtime dependency. If later adopted, dictionaries must be immutable, charged,
  reference-tracked, and preserved by backup/restore.
- Packing tiny objects or using compact database storage can avoid substantial
  filesystem allocation overhead. Measure the selected backend before rollout;
  if loose objects erase the savings, use bounded indexed packing/compaction.
  Packing must support verified reads, reference-safe deletion, and eventual
  reclamation of holes, with reserved repack space. This does not alter source
  identities or authorize cross-document sharing.

### Benchmark evidence and remaining release checks

The [native FastCDC benchmark](docs/benchmarks/native-fastcdc.md) covers 1,256
source snapshots, roughly 2–84 KB, using SHA-256, zstd-3, and compact recipes.
On the measured desktop the larger histories averaged 0.073–0.230 ms per
snapshot for encoding, excluding durable storage/database writes and CRDT
snapshot extraction. The real 50-revision README history retained about 73%
fewer source-encoding bytes with the 1 KiB target than whole-file zstd-3. After
thinning to four revisions, whole-file storage was smaller. These findings
support native server encoding; they do not establish a universal ratio or
user-capacity guarantee. The [earlier storage experiment](docs/benchmarks/history-storage.md)
also measures base-relative encoding and filesystem packing; its JavaScript
chunker must not be cited as native FastCDC throughput.

Before rollout, measure full checkpoint/reconstruction latency, physical
allocation, storage requests, queue depth and memory on deployment hardware,
including larger multi-file projects, cold objects, bursts, many independently
changing documents, and the retention tiers below. Connected users alone are
not an encoding workload measure. The current defaults (10,000 checkpoints per
deployment/hour, 300 per owner/hour, and 200 resident rooms / 512 MiB) need their
own capacity review; this proposal does not automatically raise them. Browser
offloading can be reconsidered only if measured server costs warrant its
protocol/validation complexity; WASM download size has not been measured.

### Relationship to diff display

Historical rendering, canonical HTML projection, diff calculation, redlines,
and suggestion display are specified in `SPEC-02-diff-display.md`. This
storage specification retains the source trees and assets that supply those
on-demand comparisons; it does not retain their HTML, projections, or diffs.

### Stored publication artifacts

Retain only the latest successfully published PDF per document for ordinary
viewing and download. It may predate the newest source checkpoint if later source
does not compile. Named versions, milestones, and open annotations preserve
source preferentially; none retain an older PDF. Historical comparison renders
HTML on demand and must not trigger PDF generation.

The retained PDF and its available SyncTeX/provenance companions form one
publication bundle. Charge their actual stored bytes, including existing gzip
compression of SyncTeX. Publish a verified replacement durably and atomically
switch the current registration before retiring the preceding bundle. Failed,
incomplete, or stale publication candidates must not displace the current PDF.
Uploads and active reads may temporarily lease additional objects, but those
leases are bounded and do not create an archive.

Collect superseded bundles routinely, without waiting for quota pressure and
regardless of checkpoint labels. Failed deletion remains charged until retried
successfully. The latest published bundle is part of the irreducible document
footprint until replaced or explicitly removed through the publication lifecycle.
Its registration/provenance must survive pruning of its originating source event;
retaining the PDF does not itself require retaining that source tree. Historical
PDF viewing is available only when the selected tree matches this retained bundle,
with the rendering identity explicit. Otherwise offer source inspection and
contemporary HTML comparison, not another version's PDF.

## Storage accounting

### Quota semantics

History counts against the configured owner quota. The charge is the durable
storage needed to retain that history: checkpoint and recipe metadata, unique
compressed chunks, whole-file objects, retained assets, and protected
renderings. History is not free merely because several versions share data.
Instead, shared bytes are charged once and every byte introduced solely to keep
an older version remains charged until its last reference is removed and the
underlying object is reclaimed.

Charge stored compressed length, not uncompressed length. Catalogue metadata
has a documented, deterministic byte charge based on its serialized records;
shared database page slack is deployment overhead, not attributed repeatedly
to individual checkpoints. Pending uploads and deletion queues also retain
physical bytes: keep them charged/reserved until deletion succeeds, without
charging the same object again when it becomes reachable from a committed tree.

Stored byte length is not an assertion about exact filesystem block allocation.
Measure allocator slack, indexes, shared database pages, and pack holes separately
as deployment overhead, and reserve physical disk headroom for them and bounded
maintenance. Owner byte totals must not justify filling the device past its
safe physical ceiling. Report both retained encoded bytes and allocated storage
in backend benchmarks; tiny loose objects can otherwise erase compression gains.

For example, a 1 MB text with 100 one-word revisions is charged for its initial
compressed chunks, the replacement chunks introduced by those revisions, and
100 small recipes and checkpoint trees. It is not charged as 100 complete
1 MB files, and it is not charged as only the current 1 MB file. This makes a
100 MB quota a bound on retained source history rather than a multiplier over
logical snapshot sizes.

Physical retained-byte accounting is required before byte-driven pruning under
the new model becomes authoritative. Age and count thinning do not depend on
that switch. The present logical sum may remain as a conservative admission
guard during migration, but it must not remain the long-term meaning of `--quota`.

Apply an owner's hard quota in this order:

1. Account for the live document, unique source blobs, unique assets, tree and
   catalogue metadata, and protected publication artifacts.
2. Evict regenerable caches.
3. Remove unprotected derived renderings according to their separate retention
   rules.
4. Thin routine checkpoints using the cumulative reclaim calculation below,
   including groups whose objects become unreachable only when removed together.
5. Remove protected source versions only if routine-history reclamation is
   insufficient. The current publication bundle is not a history candidate.

Checkpoint metadata itself has a measurable charged size and may justify
pruning. Never substitute the checkpoint's logical full-tree size for the bytes
actually released. Browser-local cache eviction cannot free server quota.

### Charge retained objects once

Change history quota accounting from the sum of checkpoint logical tree sizes
to the bytes of unique retained objects:

- tree manifests;
- text recipes, compressed chunks, and whole-file source objects;
- assets referenced by at least one retained tree;
- publication artifacts retained by an explicit publication policy;
- the live CRDT session and other document-owned durable state already covered
  by the quota.

A chunk or object referenced by several retained checkpoints is charged once.
The history still incurs the recipe and checkpoint metadata for each version
and every unique chunk needed only by an old version. There is deliberately no
cross-document object sharing: identical content in separate documents is stored
and charged separately within their storage identities and deletion boundaries.

Quota calculation must be based on durable catalogue/object metadata and must
not list or download every object on each checkpoint. Maintain reference and
size metadata transactionally, or compute a conservative cached total that can
be repaired from retained trees.

During rollout, compare the unique total with the existing conservative logical
total in audit mode. Continue using the old total for enforcement only until the
unique total is transactionally maintained, repairable, and verified against
the object store. Switching enforcement is part of this project rather than an
optional later optimization.

### Compression and encoding migration

Use FastCDC and zstd as chosen above; measure representative retained history
to finalize the single writer profile and small-file threshold. Add transparent
chunked and whole-file compression for new
text versions. Digest identity continues to describe canonical uncompressed
bytes; storage metadata records the encoding and stored length. Readers verify
the reconstructed content against the complete-file digest.

Benchmark the selected pipeline against native whole-file zstd across edit
locality, small files, and object/recipe overhead, before and after thinning.
Keep comparisons with dictionaries or one-hop bases as bounded experiments,
not requirements to ship a broad parameter search or multiple runtime codecs.
Measure total retained/allocated bytes and read/write costs, not compression
ratio alone.

Existing uncompressed blobs remain readable. Garbage collection, backup,
restore, and quota repair must understand both encodings before compression is
enabled.

Backups must preserve recipes and chunks or materialize verified complete files.
Restore may import either representation and must not depend on checkpoint
creation order.

### Publication and garbage-collection safety

Extend the existing collector to follow encoding records, recipes, chunks, and
legacy whole-file references. Before enabling chunk writes, establish this
publication protocol:

1. Reserve quota and register a durable in-flight write lease covering the
   intended object keys before uploading them.
2. Write and verify chunks, then recipes/encoding records, then the logical tree.
   Readers must see only committed encoding records.
3. Atomically publish the checkpoint, object sizes/references, accounting, and
   any accompanying retention changes in the authoritative catalogue. Preserve
   catalogue-only attribution such as `by_account`; never reconstruct it from
   public manifests.
4. Publish any derived manifest/cache view from that committed state. Only then
   retire the lease and schedule objects with no remaining references for GC.

GC must coordinate reference creation and deletion, recheck references/leases
before deletion, and prevent a new publisher from adopting an object already
claimed for deletion. Live documents, active restores, captured comparison reads,
retained trees, and protected artifact registrations all supply roots or leases.
A bounded renewable lease protects a slow active writer; an age grace period
alone is insufficient. Expired abandoned writes become orphan candidates only
after the grace period. Recovery repairs committed catalogue state and reclaims
abandoned objects; it must not resurrect pruned history from a stale manifest.

Retention-only transactions may precede a new write when admission requires
reclaiming space first. Where publication/compaction needs temporary duplicate
storage, use a bounded, separately reserved maintenance allowance within the
deployment ceiling; never treat projected GC savings as already free bytes.

## Retention policy

Retention selects checkpoint events; GC reclaims objects after durable reference
updates. The newest checkpoint and live session are never automatic eviction
candidates. Temporary read/restore/write leases prevent deletion during use.

### Protection classes

This intentionally expands the existing label-only preference:

| Condition | Retention class |
| --- | --- |
| Non-empty label, regardless of reason | Protected |
| `cli`, `publish`, `restore`, legacy `restored`, `accept` | Protected milestone |
| Revision referenced by an open comment thread or pending suggestion | Protected while open |
| `quiet`, `automatic`, `left`, `sync`, `render`, `recovered` | Routine unless another condition protects it |
| `comment` without an open reference; `label` without a label; unknown reason | Routine unless another condition protects it |

These are default preferences, not additional count/byte exemptions. Where
`SPEC-quota-preferences.md` allows a user to disable a milestone preference,
that condition no longer protects the event; any other enabled protection still
applies. Treat explicit `cli` milestones as a separate preference, not all sync
or autosave activity. Even named or open-annotation history remains subject to
hard count and byte limits.

An open annotation protects its referenced revision even if the checkpoint's
reason is not `comment`. When several events name that revision's tree, protect
an explicitly referenced event, or the newest matching retained event for a
tree-only reference. Resolving, rejecting, accepting, or deleting the last open
annotation releases that preference; reopening restores it only if the revision
is still retained. Maintain these references from durable annotation state.
Protection is preferential under hard limits, including for open annotations.
If their revision is evicted, keep their stored quotations and report unavailable
history; never silently bind them to another revision.

### Age tiers and count limits

The default (Balanced) profile is deliberately storage-conscious; it does not
keep every checkpoint for 24 hours. At one captured evaluation time, select the
newest routine event in each occupied bucket within the following age tiers:

| Checkpoint age | Routine retention density |
| --- | --- |
| Less than 1 hour | One per UTC-aligned 5-minute bucket |
| At least 1 hour, less than 24 hours | One per UTC clock-hour bucket |
| At least 24 hours, less than 7 days | One per UTC-aligned 6-hour bucket |
| At least 7 days | One per UTC calendar-day bucket, subject to count/byte limits |

For bucket width `w` seconds, use `floor(timestamp / w)` with widths 300, 3,600,
21,600, and 86,400, respectively. UTC midnight anchors six-hour and daily buckets.
Age intervals are half-open as shown. A five-minute bucket is a density rule,
not a promise that retained events are at least five minutes apart: events near
opposite sides of a boundary can be close together. No owner timezone field or
daylight-saving rule affects selection; user-facing timezone is presentation only.

This yields roughly 60 routine checkpoints in the first week of dense activity
(nominally 12 + 23 + 24; at most 62 occupied tier/bucket intersections because
evaluation time may cut boundary buckets). Protected events and temporary active
leases can add entries, and older daily history continues until count/byte or
configured duration limits apply. This is not a bound on total checkpoint count
or physical bytes. Empty or previously pruned buckets are not backfilled.

Choose bucket winners from the routine events in that age tier, using timestamp,
then catalogue sequence, then event SHA to break ties. Protected events survive
age thinning independently. Apply routine bucket selection as part of checkpoint
publication/its retention transaction so repeated saves do not accumulate an
entire day of dense committed history awaiting a daily sweep. Protect leased
events until use ends and bound deferred cleanup/admission. Also run bounded
maintenance so inactive documents age into sparser tiers. Never overwrite an
immutable event in place to update a bucket winner.

User-selectable profiles and overrides in `SPEC-quota-preferences.md` may adjust
these densities only within deployment bounds. The backend resolves one effective,
versioned policy used by admission, previews, maintenance, and the UI.

These are preferences, not guarantees under hard count or byte limits. Candidate
classes, from first removed to last, are: routine non-winners, routine tier
keepers (including recent checkpoints), then protected checkpoints. Within each
class remove oldest first, with catalogue sequence then event SHA as tie-breakers.
For owner-wide quota selection across documents, use timestamp, storage identity,
catalogue sequence, and event SHA to make ties total; exclude each document's
newest checkpoint.

Enforce a configured count cap with the shortest candidate prefix that meets it;
it may evict labelled/protected checkpoints after routine events are exhausted.
`--history 0` retains only the newest checkpoint. An unset CLI cap remains
uncapped by count. Age thinning still applies. Byte and count limits are independent.
If active leases temporarily prevent meeting a cap, defer new checkpoint admission
or the affected maintenance pass until they end; do not break an active restore
or comparison to force the count down.

### Byte-driven selection and admission

For a candidate set `S`, define `reclaimable(S)` as the charged event/catalogue
metadata removed plus the stored bytes of objects whose last reference is
removed by `S`. Count each object once, including recipes, chunks, trees, assets,
and any artifact bundles retired with the set. Exclude live roots, leases, and
independently protected artifacts. A tree may be shared by multiple events;
neither a unique tree nor unique source bytes per event can be assumed.

After earlier artifact-eviction steps, scan cumulative prefixes of the ordered
checkpoint candidates. Choose the shortest prefix whose reclaimable bytes cover
the required deficit. Exhaust routine candidates before the protected tier.
Superseded publication bundles have already been scheduled for independent
reclamation; neither labels nor this ordering prolong their retention.
The selected set must have positive total reclaimable bytes. Individual members
may have zero marginal object bytes: if A and B alone reference a large old asset,
`reclaimable({A, B})` includes it even when neither singleton does. Use catalogue
reference metadata for this calculation, not object-store listings. Count-only
thinning does not require positive byte reclamation.

Plan admission before deleting history for a proposed write. If no permissible
set can make it fit, refuse/defer the growth with an actionable quota error and
leave the committed session/newest checkpoint intact. If a lowered quota leaves
even the irreducible live footprint over limit, report that condition and refuse
growth; do not delete protected history in a futile attempt to make it fit.
Allow reads, export, and space-reducing operations. Concurrent admissions must
reserve bytes transactionally. Planned GC savings become available quota only
after successful deletion; a slow or failed sweep cannot authorize unbounded
new writes.

At fixed time, references, configuration, and leases, repeating a completed pass
removes nothing further. Advancing time, resolving annotations, or releasing
leases is a state change that may enable another pass.

When a checkpoint is removed:

- reparent its direct successor to the removed checkpoint's parent for timeline
  continuity, preserving its original parent and recording an ancestry gap;
- recompute the successor's `changed` paths from its new retained parent tree,
  or explicitly invalidate the summary and require comparison of the two trees;
- preserve checkpoint actor identity; reparenting must not imply that actor
  authored all changes across the new gap (see `SPEC-02-diff-display.md`);
- delete its catalogue/manifest entry atomically;
- delete its tree object only when no retained event or other protected root or
  active lease references that tree;
- delete recipes, chunks, whole-file source objects, and assets only when
  neither a retained tree nor the live document references them;
- delete derived artifacts only when no protected policy retains them;
- leave every remaining checkpoint independently readable and restorable.

Existing histories without provable original adjacency are marked unknown rather
than retrospectively claiming an unbroken authorship chain.

Named versions should normally survive thinning, but naming is not an unlimited
storage guarantee. The UI and documentation must say that deployment storage
limits still apply.

## Checkpoint cadence

Retain the existing scheduling policy unless measurements justify changing it:

- checkpoint after 30 seconds without edits when content changed;
- checkpoint after at most five minutes of continuous uncheckpointed editing;
- coalesce checkpoint requests that occur within the existing defer window;
- checkpoint meaningful actions immediately under their existing rules;
- do not create a new content event for unchanged routine state.

These defaults are `CHECKPOINT_QUIET_SECONDS = 30` and
`CHECKPOINT_MAX_INTERVAL_SECONDS = 300`. `--checkpoint N` overrides the quiet
period in minutes; correct its stale "default 5" help text as part of rollout.
The existing 30-second defer window and owner/deployment rolling-hour admission
budgets remain separate controls. Budget exhaustion may defer automatic work;
the five-minute scheduling target does not bypass admission limits.

Live editing/session durability is separate from history checkpoint creation
and retention. Keep its existing prompt persistence/acknowledgement rules;
thinning must not turn ordinary saving into a five-minute or hourly operation.
Routine checkpoints may initially be captured more often than the retention
bucket width, but only the newest eligible event per bucket survives selection.
The UI must not describe every saved state as an indefinitely available version.

The tier policy controls recent as well as old retained history; do not retain
a hidden duplicate archive for recovery, attribution, or bucket replacement.
Leased/grace-period exceptions are bounded and charged. Changing capture cadence
further is a separate measured optimization, not required for this retention
change. Automatic checkpoint delay and durable-live-save status must remain
distinguishable when admission or the encoding queue is saturated.

## Migration

No eager source-history rewrite is required. Preserve complete-tree histories,
legacy single-file checkpoints, and `sources/<slug>` reads.

Deploy in stages:

1. Add encoding-independent catalogue lookups and mixed-format readers. Extend
   backup, restore, repair, and existing GC with references and in-flight leases
   before enabling compressed/chunked writes. Freeze the compact recipe format,
   select the measured writer profile/threshold, and enforce native worker and
   queued-byte limits before enabling server encoding. No browser codec rollout
   is required.
2. Add retained-object accounting in audit mode. Exercise new encodings while
   the old conservative guard still controls admission.
3. Switch `--quota` enforcement and byte-driven candidate selection together
   after totals are accurate, transactionally maintained, and repairable.

Age/count thinning may ship independently, once protection classes, ancestry-gap
metadata, and safe deletion are implemented. It must not pretend to enforce the
new physical quota while logical full-tree sums remain authoritative.

Changing existing histories to the tighter Balanced profile is destructive.
Preview/audit the affected events and reclaimable bytes and use the confirmation
and bounded grace process in `SPEC-quota-preferences.md` (or equivalent explicit
operator migration approval). Do not silently interpret old "keep all recent"
settings as the new default on upgrade. New histories use the new profile;
existing policy revisions remain interpretable until explicitly migrated.

Switch publication retention to latest-only once historical comparison and
passage lookup use HTML or an explicit source fallback rather than requiring
stored PDFs. Then retire existing older bundles, including labelled ones, through
the normal lease-aware collector. This cleanup does not require chunked storage
and must preserve the latest successfully published bundle throughout rollout.

Existing uncompressed source blobs remain readable throughout rollout. Backup,
restore, garbage collection, and integrity verification must support both
encodings before new writes switch to chunked storage.

## Observability

Record aggregate measurements without document content:

- checkpoints created, retained, thinned, and protected by reason;
- logical tree bytes and unique physical object bytes per document;
- compressed bytes introduced per checkpoint, chunk reuse ratio, recipe
  overhead, and whole-file fallback rate;
- garbage-collection candidates, reclaimed bytes, and repair discrepancies.
- encoding/reconstruction latency by source-size class, cold versus reused
  objects, queue age/depth/bytes, worker saturation, and rejected/deferred work;
- actual filesystem allocation/pack slack, storage-request counts, reserved
  maintenance space, and fallback objects awaiting optional compaction;
- retained routine counts by tier, temporary lease/grace exceptions, and live
  durability versus checkpoint-admission delay.

Emit structured server logs and benchmark/audit reports initially; this project
does not require a new metrics endpoint. Use opaque document identifiers and
exclude source text, file paths, credentials, and annotation quotations.

These measurements determine chunk parameters, retention effectiveness, and
whether the chosen encoding materially reduces charged history storage.

## Acceptance criteria

1. Every retained checkpoint remains directly readable and restorable after
   arbitrary intermediate checkpoints are thinned.
2. A local edit to a large single-file document stores new chunks around the
   edit and reuses unchanged chunks before and after it; it does not store a
   second complete file object.
3. Reconstructing a chunked file verifies its complete uncompressed digest and
   detects a missing, reordered, or corrupt chunk.
4. Small text uses whole-file zstd below the measured threshold; large text uses
   FastCDC plus independently compressed chunks. Assets/binaries stay whole.
   Threshold evidence includes recipes and retained history, not only payloads.
5. History charges include checkpoint metadata and every compressed chunk,
   recipe, whole-file object, asset, and protected artifact retained solely or
   jointly by its checkpoints.
6. Byte pruning measures cumulative unique retained bytes, including metadata;
   it can reclaim an object shared only by several jointly removed events and
   never credits the logical full-tree sum or a failed physical deletion.
7. Regenerable caches and unprotected derived artifacts are evicted before
   source checkpoints under quota pressure.
8. The newest checkpoint is never removed. Routine retention follows the
   five-minute/hourly/six-hourly/daily age tiers with deterministic UTC buckets;
   there is no default 24-hour keep-everything window. Hard caps override tier preferences and then
   protection in the specified order, including count caps and `--history 0`.
9. Garbage collection never removes an object referenced by the live document,
   a retained checkpoint, an active lease, or a protected rendering.
10. Existing histories and uncompressed blobs require no eager migration and
    remain readable throughout rollout.
11. Focused tests cover stable content-defined chunk boundaries, complete-file
    digest verification, pruning boundaries, shared-chunk accounting, garbage
    collection, backup and restore, mixed old/new encodings, and crash recovery
    between recipe, chunk, tree, and catalogue updates.
12. Re-encoding a file leaves its logical tree digest and rendering/label links
    unchanged. Legacy pre-tree and `sources/<slug>` histories remain readable.
13. Tests cover 101 recent events under a cap of 100, all-labelled histories,
    shared-tree events, joint asset reclamation, annotation resolve/reopen,
    UTC boundaries, fixed-time idempotence, and irreducible over-quota documents.
14. Concurrent GC cannot delete leased writes or restores; publication, pruning,
    retries, and crashes cannot double-charge or prematurely release bytes.
15. Reparenting invalidates or updates changed-path summaries, preserves
    catalogue-only attribution, and exposes ancestry gaps to comparison clients.
16. After leases and GC complete, only the latest published PDF and its companion
    objects remain, even with many named checkpoints. Failed/stale publication
    preserves the prior bundle; successful replacement retires it without quota
    pressure. Tests cover migration of labelled PDFs and pruning the current
    bundle's originating event without losing its registration.
17. Production encoding uses the pinned FastCDC implementation, zstd, SHA-256,
    and compact versioned recipes. No custom chunker or browser encoding module
    is required; decoding never requires the original chunker or writer parameters.
18. A multi-editor document produces one encoding job per document/file identity.
    Worker, queue-byte, memory, and storage-concurrency limits hold under burst
    and large-input tests; CPU work does not hold room locks or block async tasks.
    Overload defers routine history without falsely acknowledging a checkpoint
    or weakening live-save durability. Fallback cannot exceed its byte/work budget.
19. Tests cover exact 1-hour/24-hour/7-day transitions, five-minute and six-hour
    boundaries, partial buckets, empty buckets, same-time ties, timezone/display
    changes, and the first-week routine-count bound absent protection/leases.
    Publishing multiple routine events in one bucket leaves only the newest
    eligible winner after lease-safe cleanup. Saving edits remains independent.
20. Re-encoding/packing tests account for shared references, net savings, reserved
    temporary space, concurrent readers, atomic switches, and eventual reclamation;
    no optional compactor is required to recover a committed source version.

## Open questions

- Which single FastCDC size profile and small-file threshold best balance
  retained bytes, recipes, physical allocation, and read/write costs under the
  new retention tiers on representative projects?
- Does the deployment backend require indexed packing or compact database
  storage at first rollout to avoid losing the measured savings to allocation
  overhead, and what measured worker/queue budgets fit that deployment?
