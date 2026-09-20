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
| 1 | Remaining admission and capacity questions | Some configured limits are unenforced, and contention still needs measurement | Decide which limits to enforce and verify database capacity under load |
| 2 | Companion and assistant scope | Potentially large cuts, but some remove useful entry points | Confirm unused paths; decide which local and agent workflows to support |

History truncation, narrower offline support and new compaction architectures
are conditional options, not prerequisites. Companion and assistant cuts need
a clear product decision before removing supported workflows.

The original idea numbers are retained in parentheses below so references to
this review remain understandable.

## 2. Highest-value remaining work

### 2.1 Admission and capacity gaps (remaining original 2.7)

The [resource inventory](docs/resource-bounds.md) identifies these outstanding
questions. Each needs a product decision rather than a quiet change to the
supported limits:

- `config.rate_per_hour` (comment rate, 20/hour) is defined but not enforced.
  Comments are bounded only by the general request bucket (6,000/minute), for
  both authenticated and commenter-link callers.
- `config.max_comments` (500) and `config.max_replies` (100) are not enforced.
  `PostgresCatalog::annotation_count` exists and has no caller; a comment in
  `room/comments.rs` asserts the cap that is not applied. Annotation and
  reply counts are unbounded in practice.
- `config.session.label_deployment_per_hour` (10,000) reaches no check; only
  `label_owner_per_hour` does.
- The MCP `Capacity` semaphores are one global instance, not per document or
  per principal, so one runaway agent loop can starve MCP traffic for the
  whole deployment.
- The retention janitor, when an operator turns retention on, issues a
  catalogue read every hour whether or not anything expired. Decide how to
  reconcile this behavior with the idle-query rule or document its exception.

**Still unverified:** that 20 PostgreSQL connections suffice under 64
concurrent HTTP work slots plus background work. Measure this under load.

**Remaining end-to-end coverage:** recovery after a background scan loses its
database connection, and discovery of work created behind the cursor during a
real database scan. The existing map and queue tests do not establish those
end-to-end behaviors.

### 2.2 Companion and assistant scope (original 2.9)

This area offers substantial possible deletion, but mixes obsolete code with
product choices. The current simplification audit notes that companion
presets, bindings and isolated workspaces still serve local builds and projects
with unshared resources. Their removal needs a replacement contract.

**Start with confirmed obsolete paths.** The original review identified
protocol v1, LaTeX discovery flags/log parsing and non-Quarto builder paths as
candidates. Verify their current callers before deleting them. Browser rendering
is not automatically a substitute for a local build that uses private files,
packages or execution.

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

1. Isolate export of a fixed durable prefix from live editing where lock
   contention is demonstrated. Budget the independent document and bounded
   blocking work; cloning a Loro handle does not isolate its state.
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

### 3.5 One active editor, no CRDT (original 2.12)

One active editor with revision-checked saves, everyone else submitting comments
or patches, would remove the CRDT integrations, live merge stack and much offline
reconciliation. It is the largest reduction and the largest change to what
LibrePaper is. Neither original review recommended it. Revisit only if
simultaneous editing itself stops being a product requirement.

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
