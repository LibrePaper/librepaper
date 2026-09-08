# 3. Shared retention traversal

Status: proposed. Inherits [umbrella section 3](../../../SPEC-refactor.md#3-share-history-traversal-and-tree-loading-during-pruning).

## Scope and implementation

Build one pass-scoped plan for asset, blob, and rendering pruning from a
consistent paginated view of complete retained history. Reuse event metadata
for renderings without requiring tree reads. Decode each distinct tree once,
extract text/asset references, and release its body immediately.

Use bounded tree-read concurrency and document memory costs of the accumulated
identity/reference sets. Pagination alone does not bound those sets. Tie the
plan to the manifest/history version and use the established lock order from
track 2. Invalidate or conservatively skip deletion when revalidation fails.

Keep final deletion checks for live references, new checkpoints, uploads,
restores, labels, and rendering grace periods. An unreadable tree makes deletion
unsafe for anything it could reference; a partial plan is not complete evidence.

## Delivery and acceptance

Deliver plan construction and its pruning consumers together. Keep retention
policy and event-versus-content identity unchanged.

- Storage counters demonstrate one history traversal per stable pass and at
  most one read per unique retained tree; record baseline counts for comparison.
- Cover histories above 64 entries, repeated content, concurrent labels,
  restores/uploads, missing trees, and failed history pages.
- Inject failure between manifest persistence and deletion. Uncertain inputs
  preserve objects; confirmed physical deletion alone releases accounting.
- Show bounded concurrent decoded tree bodies and document reference-set growth.

## Implementation evidence

Checkpoint cleanup now calls one `prune_retained` pass. It holds the existing
manifest gate while reading the paginated history and running its consumers,
so labels and checkpoint graph mutation cannot make the plan stale. It acquires
neither room state nor an asset/rendering gate during tree I/O. Final asset
checks still protect current references, in-flight uploads, and grace periods;
text pruning refreshes live references after listing. Checkpoint serialization
continues to exclude concurrent text-object writers.

Trees are deduplicated by content identity, read with concurrency four, and
discarded as soon as text/asset references have been extracted. The pass retains
checkpoint metadata and digest sets, whose memory grows with retained history
and distinct referenced objects; it does not retain an unbounded decoded-tree
cache. Rendering consumers reuse metadata even when a tree read fails, while
asset/text consumers skip deletion on incomplete dependency evidence.

The deterministic workload has 201 events over nine unique trees, crossing both
the 64-entry resident tail and 200-entry SQL page. It measures two catalogue
operations for one traversal, nine tree reads, and peak read concurrency four.
The previous three consumers required three history traversals and independently
loaded the trees for asset and text references. The same regression protects a
source edit arriving during blob listing and verifies that a missing retained
tree leaves potentially referenced text and assets intact.

The content-identity accessor extraction is a separate track 8 commit and keeps
legacy fallback and event identity comparisons distinct. Track 1 still owns the
asynchronous catalogue call-site migration.
