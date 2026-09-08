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
