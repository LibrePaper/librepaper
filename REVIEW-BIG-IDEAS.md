# Big architectural ideas

*Originally merged from two architecture reviews on 2026-09-19; reorganized
and reassessed on 2026-09-20 after the server-as-log cutover. These are
recommendations for decision, not an approved implementation specification.*

The brief is robustness, cost and simplicity, including selective loss of
functionality where it buys a large structural improvement. Preserve live
collaboration, review and recoverable work. Judge a simplification by the
responsibilities and failure modes it removes, not just the files it splits or
the lines it deletes.

## 1. Priorities and implementation order

| Priority by payoff | Idea | Why it matters | Recommended next step |
|---|---|---|---|
| 1 | Bounded comment transport and admission | SQL pagination fixes row ceilings, but the full-snapshot protocol still needs a read memory guard | Add end-to-end pagination for larger collections; decide admission limits separately |
| 2 | Remaining admission and capacity questions | Label limits, MCP fairness and database contention remain unresolved | Address independently after the comment correctness gap |
| 3 | Companion and assistant scope | Potentially large cuts, but some remove useful entry points | Remove proven unreachable remnants; decide which active workflows to support |

History truncation, narrower offline support and new compaction architectures
are conditional options, not prerequisites. Companion and assistant cuts need
a clear product decision before removing supported workflows.

The original idea numbers are retained in parentheses below so references to
this review remain understandable.

## 2. Highest-value remaining work

### 2.1 Admission and capacity gaps (remaining original 2.7)

The [resource inventory](docs/resource-bounds.md) identifies these outstanding
questions. The configured comment limits need an explicit enforcement policy;
the other items should not be bundled into the same change:

- `config.rate_per_hour` (comment rate, 20/hour) is defined but not enforced.
  Comments are bounded only by the general request bucket (6,000/minute), for
  both authenticated and commenter-link callers.
- `config.max_comments` (500) and `config.max_replies` (100) are not enforced.
  `PostgresCatalog::annotation_count` exists and has no caller. Annotation and
  reply writes are unbounded in practice. SQL reads now page beyond the old
  500-annotation and 5,000-reply ceilings, and command lookup is by document
  and ID inside the transaction. However, browser snapshots, exports and agent
  query captures still collect the result. The loader therefore refuses a
  full snapshot above a 16 MiB memory estimate, with an explicit error rather
  than an empty list. This is a read guard, not a new write-admission policy.
- `config.session.label_deployment_per_hour` (10,000) reaches no check; only
  `label_owner_per_hour` does.
- The MCP `Capacity` semaphores are one global instance, not per document or
  per principal, so one runaway agent loop can starve MCP traffic for the
  whole deployment.

**Still to implement: bounded transport pagination.** Carry cursors through
HTTP, WebSocket refresh, browser rendering, export and agent query consumers
so collections larger than the snapshot budget remain readable without
building one complete vector. Preserve authorization, comment versions and
concurrent refresh semantics. Paging only the SQL queries does not complete
this work. The existing row-boundary and transaction-lookup regressions should
continue to pass.

**Still to decide: comment admission.** Applying the comment/reply limits
inside the existing authorized document transaction, across browser and agent
entry points, makes a comment refusable, which is a product decision rather
than a correctness fix and is not implemented. Whoever takes it must settle:
what a caller at the cap is told and whether the refusal is retryable; that a
retry of an already-created UUID at the cap still succeeds; that concurrent
requests cannot both consume the last slot; and that an over-limit document
from before the change is not made unwritable in a way that strands work.
Rate limiting (`config.rate_per_hour`) is a separate change again, once its
principal, window and retry semantics are specified.

**Lower-priority operator documentation:** retention is opt-in and performs an
hourly catalogue read even when nothing expires. The resource inventory
already records this; clarify the exception to the idle-query promise in the
operator-facing retention documentation. A new scheduler is not justified by
this observation alone.

**Editing sweep improved; mixed-load capacity still unverified.**
[docs/postgres-capacity.md](docs/postgres-capacity.md) inventories database
consumers and describes the benchmark. Socket edits run after the HTTP work
permit is released, so they add load beyond the 64 request slots. The sweep
now flushes independent documents concurrently, bounded by half the pool,
while retaining each document's transaction gate. Its regression test checks
both overlap and the bound against PostgreSQL.

The original throughput tables counted setup rows and mixed workload and drain
intervals; their capacity conclusions are withdrawn. The corrected harness
counts scheduled demand independently of ingest, includes scheduling delay in
latency, and separates workload and drain measurements. Neither HTTP traffic
nor background compaction is exercised, so whether twenty connections suffice
for their combined load remains open. No numerical deployment envelope or
pool-size recommendation follows from these isolated runs.

**Next, on the same shape:** background maintenance is still one task, so one
compaction at a time blocks every other document's compaction, deletion and
archive, and a document past `log_quota_bytes` refuses edits until its turn
comes. Document-open and reconnect bursts, which *are* under the work
semaphore and do cost about six queries each, are the other unmeasured case.
Both are named in section 8 of the capacity document.

**Remaining end-to-end coverage:** recovery after a background scan loses its
database connection, and discovery of work created behind the cursor during a
real database scan. The existing map and queue tests do not establish those
end-to-end behaviors.

### 2.2 Companion and assistant scope (original 2.9)

This area offers substantial possible deletion, but mixes obsolete code with
product choices. The current simplification audit notes that companion
presets, bindings and isolated workspaces still serve local builds and projects
with unshared resources. Their removal needs a replacement contract.

**Start with remaining legacy remnants.** LaTeX-specific discovery fields and
diagnostic parsing branches remain candidates for removal after checking their
callers and persisted configuration. Do not treat all non-Quarto builders as
obsolete: Typst, Pandoc and Calepin still have executable builder paths, and
the browser exposes the protocol-v2 build route. Removing them is a supported
workflow decision. Browser rendering is not automatically a substitute for a
local build that uses private files, packages or execution.

**Choose the assistant entry point.** Remote MCP and the stdio bridge provide
an agent path already. The sidebar additionally brings an ACP runner, task
journal, lifecycle supervision and server chat relay. Options are to keep that
integrated experience, make the sidebar a thin local client, or support
external agents only. Removing it may save substantial maintenance, but also
removes the easiest entry point for people without a separate coding agent.

**Other bounded candidates:**

- Compare the roughly 2,100-line `agent_query` module with a smaller source and
  search interface. Retain it only if its specialized operations materially
  improve real editing tasks.
- Consider build-on-save through one job path instead of a separate watched
  preview lifecycle. Specify the preview freshness users would lose.
- Simplify presets and grants only after preserving the required local build
  inputs and execution consent. Removing frozen-cache checks must not silently
  authorize collaborator-supplied code.
- A copied personal API token could replace device login, its cache and CLI
  polling. This trades onboarding convenience for less code and is lower
  priority than document and review work.

**Done when:** supported browser, local-build and agent workflows are named;
retired paths and their dependencies are removed on both sides of the protocol;
and the retained workflows still have a usable first-run experience. Avoid
shipping a second implementation beside the one intended for removal.

## 3. Conditional options to revisit with evidence

### 3.1 Bounded collaboration history (original 2.3)

Separate two promises: saved versions remain readable/restorable as plain
project snapshots, while automatic CRDT merging works only within the current
collaboration generation. At an explicit maintenance boundary, start a fresh
generation from current source. Returning older clients retain their work and
receive a recovery copy or reviewed patch. Comments retain their original
quotation and snapshot reference, with an explicit unplaced state where an
anchor cannot be recovered.

**Revisit when:** representative long-lived documents hit unacceptable replay,
memory or storage costs despite the current limits and compaction. Any history
boundary must account for open proposal branches that still need old history.

**Sacrifice:** seamless merging after arbitrarily long absences, perpetual
operation-level blame and access to every intermediate keystroke. This is a
retention and recovery product decision, not transparent storage maintenance.

### 3.2 Narrower offline support (original 2.4)

A smaller contract preserves unsynced typing locally and supports reconnect,
but requires connectivity for file creation, deletion and renaming. Failed
reconciliation offers recovery or export. Restricting a project to one editing
tab per browser profile is a separate, optional product restriction.

**Revisit when:** an inventory shows substantial directory reconciliation,
cache migration or multi-tab coordination machinery can actually be deleted.
Simply disabling controls while retaining all reconciliation paths does not
justify the feature loss. Account separation and recovery of unsynced work
remain necessary under either contract.

**Sacrifice:** offline directory editing and, if chosen, simultaneous local
tabs. Do not narrow the promise merely to make the interface look simpler.

### 3.3 Compaction and history storage (original 2.13)

Full-history snapshots can repeatedly rewrite a document's past as its tail
grows. Revisit storage changes only if representative document shapes and
concurrent workloads demonstrate unacceptable costs. Use the existing
`storage::postgres::benchmarks` harnesses to investigate transient memory and
editing latency under load before choosing a new architecture.

**If measurements justify more work, proceed in this order:**

1. Isolate export of a fixed durable prefix from live editing only where lock
   contention is demonstrated. Export already uses bounded `spawn_blocking`
   work, but holds the sequencer lock while exporting a cloned Loro handle.
   Any independent document must fit the memory budget; cloning the handle
   does not isolate its state.
2. Tune compaction triggers to tail/base proportions and a recovery-time bound
   if small tails cause excessive full rewrites. Preserve startup scheduling
   and safe coordination between base activation, joining readers and deletion.
3. Consider immutable history segments with less frequent loading snapshots
   only if repeated rewrites still dominate. Segments introduce indexing,
   recovery and garbage-collection machinery.

Coverage must be verified before deleting source rows. Truncating active
history, including using shallow snapshots, additionally requires the explicit
generation/recovery policy in §3.1. It is not an interchangeable snapshot format.

### 3.4 One rendered artifact per document (original 2.11)

Reader-side rendering moves computation and renderer downloads to every reader;
LaTeX rendering also depends on a third-party mirror. If measured reader startup
or repeated rendering becomes a problem, consider one current rendered artifact
uploaded by an editor and replaced after edits settle.

Keep it derived and disposable, keyed to the source identity so stale output is
visible. Define supported formats, artifact isolation and authorization before
serving it. This could improve reading without restoring a full publication or
rendered-version subsystem, but adds artifact upload and freshness handling.

## 4. Contracts to preserve throughout

- Transactional source changes and review decisions, with authorization checked
  at the commit boundary.
- Honest save indicators based on durable coverage, with recoverable unsynced
  work across disconnection and restart under the supported recovery contract.
- Document and account isolation, including origin boundaries for rendering.
- The single-writer deployment lease and one sequencing boundary per document.
- Bounded memory, queues, concurrency and storage, even when billing and policy
  machinery becomes simpler.

For each proposed cut, name the user promise being relaxed, the machinery that
will disappear and the evidence that the retained behavior works. A second path
added beside the old one is unfinished simplification.
