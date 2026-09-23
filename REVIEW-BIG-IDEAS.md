# Big architectural ideas

*Originally merged from two architecture reviews on 2026-09-19; reorganized
and reassessed on 2026-09-20 after the server-as-log cutover. These are
recommendations for decision, not an approved implementation specification.
Pruned on 2026-09-21 against the implementation and recorded measurements.*

The brief is robustness, cost and simplicity, including selective loss of
functionality where it buys a large structural improvement. Preserve live
collaboration, review and recoverable work. Judge a simplification by the
responsibilities and failure modes it removes, not just the files it splits or
the lines it deletes.

## 1. Priorities and implementation order

| Priority by payoff | Idea | Why it matters | Recommended next step |
|---|---|---|---|
| 1 | Comment admission | Transport is now paged end to end; what one document may accumulate is still unbounded, and making a comment refusable is a product decision | Settle what a caller at the cap is told, and whether a retry of an already-created id still succeeds |
| 2 | Remaining admission and capacity questions | Label limits and database contention remain unresolved | Address independently |
| 3 | Unused companion and agent paths | Verified legacy remnants may offer bounded deletions | Check callers and persisted configuration; preserve supported workflows |

The speculative architecture backlog has been removed: the recorded compaction
costs do not justify history replacement, and no demonstrated deletion or
reader-performance benefit justifies reducing offline support or introducing
rendered-artifact uploads. New proposals need a concrete problem and evidence
of a net reduction in cost or complexity.

The original idea numbers are retained in parentheses below so references to
this review remain understandable.

## 2. Highest-value remaining work

### 2.1 Admission and capacity gaps (remaining original 2.7)

Unenforced configuration keys were removed on 2026-09-23 rather than left as
documentation of intent: `config.rate_per_hour` (comment rate), `config.max_comments`
and `config.max_replies` (comment and reply admission), and label-creation limits.
Comments and replies are bounded only by the general request bucket (6,000 per
minute per principal), and versions are bounded by the pending-source budget.

**Done: bounded transport pagination.** Every consumer walks a keyset
traversal a page at a time: HTTP, the socket `hello` and its invalidation
frame, the browser panel, `librepaper export` and the agent `thread` query.
The whole-snapshot protocol and its 16 MiB read guard are gone, along with
the room's resident copy of every comment; what a room keeps is a bounded
cache of where anchors resolved to. Page sizes, cursors, the consistency
contract during concurrent change and the remaining aggregate limits are in
[docs/protocol/comments-v1.md](docs/protocol/comments-v1.md).

**Decided against: comment admission.** Enforcing the comment and reply
ceilings inside the authorized document transaction was implemented and
tested on 2026-09-23 and then dropped in favour of the knob cull above: a
comment stays unrefusable, and comments and replies remain bounded only by
the general request bucket. Anyone revisiting it starts from the same four
questions, none of which this decision answers: what a caller at the cap is
told and whether the refusal is retryable; that a retry of an
already-created UUID at the cap still succeeds; that concurrent requests
cannot both consume the last slot; and that an over-limit document from
before the change is not made unwritable in a way that strands work. Rate
limiting is a separate change again, once its principal, window and retry
semantics are specified. What remains unbounded is accumulated storage per
document, which pagination bounds the reading of but not the growing of.

**Done: operator documentation for retention.** Retention is opt-in and
performs an hourly catalogue read even when nothing expires. The exception
to the idle-query promise is now written down where an operator will meet
it, in [docs/hosting.md](docs/hosting.md#resource-limits), with a cross
reference from the resource inventory. A new scheduler is not justified by
this observation alone.

**Editing sweep improved; mixed-load capacity still unverified.**
Socket edits run after the HTTP work
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

**Next, on the same shape: the harness now exists, the numbers do not.**
Background maintenance is still one task, so one compaction at a time blocks
every other document's compaction, deletion and archive, and a document past
`log_quota_bytes` refuses edits until its turn comes. Document-open and
reconnect bursts, which *are* under the work semaphore and do cost about six
queries each, are the other unmeasured case. `storage::postgres::benchmarks`
now carries a scenario for each of the three, plus mixed HTTP, editing and
compaction traffic against one pool; each documents its own shape at the top
of its function. None of them has been run, so every figure they print is a
placeholder: measure before adding worker concurrency or raising the pool,
which is the order this review asks for.

**Done: end-to-end scan coverage.** Recovery after a background scan loses
its database connection, and discovery of work sitting behind an advanced
cursor, are both covered against PostgreSQL in
`storage::worker_recovery_tests`. The second found a real gap. Ids are uuid
v7, so a row created during a pass always sorts above the cursor, but a row
created long before it can still become work while the pass runs, and its
low id is then behind a page already consumed. Every write that creates such
work also asks the worker directly, except during the very first scan after
a restart, so the worker now runs one unconditional follow-up pass from a
fresh cursor at startup and never scans on its own again.

### 2.2 Companion and agent interface cleanup (original 2.9)

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

Keep the current agent query interface and local-build abstractions. Module
length alone does not establish that a smaller interface would support editing
as well, and the audit identifies live consumers of presets and workspaces.
Limit this cleanup to verified unused paths; execution consent remains covered
by the [security contract](docs/specs/SPEC-security.md).

**Done when:** supported browser, local-build and agent workflows are named;
retired paths and their dependencies are removed on both sides of the protocol;
and the retained workflows still have a usable first-run experience. Avoid
shipping a second implementation beside the one intended for removal.

## 3. Contracts to preserve throughout

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
