# Frugal scaling: remaining validation

## Size the target deployment

Run the [sustained-write and mixed-workload harnesses](tools/frugal-validation/REPORT.md)
on the intended small VPS, with clients on a separate host and representative
project sizes. Measure server RSS separately from client memory, PostgreSQL
memory, per-update durable-save latency, and interactive request tails. Use
those results to choose pending/scratch budgets and admission limits; local
synthetic measurements cannot size the deployment.

Exercise a crowded document within its admission limits and reconnect after
an actual server restart. Include cold replay time and browser retry/backoff
behavior. The current reconnect harness keeps the server and room cache warm.

Only pursue a focused authorization lookup, writer-probe amortization, or
flush prioritization if these measurements identify a material bottleneck.
Preserve permission checks, writer fencing and durable acknowledgements.

## Complete representative storage and recovery coverage

Run the [read-only inventory](tools/frugal-storage-inventory/README.md) on a
representative current-schema deployment, including physical object bytes,
indexes, WAL and retained backup copies. The starter fixture cannot establish
production deduplication savings or history costs.

Extend the [encrypted recovery drill](tools/frugal-recovery/README.md) to cover
live and retired snapshots, archived history, and proposal payloads; the
current fixture contains none of these. Verify application replay and object
integrity after restore, then record recovery time against the deployment's
recovery objectives. Establish operator-owned key custody, backup scheduling
and retention before relying on this procedure operationally.
