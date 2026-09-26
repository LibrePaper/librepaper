# Big architectural ideas

*Originally merged from two architecture reviews on 2026-09-19; reorganized
on 2026-09-20 after the server-as-log cutover, pruned on 2026-09-21 against
the implementation, and cut back on 2026-09-23 to the one idea still open.
These are recommendations for decision, not an approved implementation
specification.*

The brief is robustness, cost and simplicity, including selective loss of
functionality where it buys a large structural improvement. Preserve live
collaboration, review and recoverable work. Judge a simplification by the
responsibilities and failure modes it removes, not just the files it splits or
the lines it deletes.

The speculative architecture backlog was removed earlier: the recorded
compaction costs do not justify history replacement, and no demonstrated
deletion or reader-performance benefit justifies reducing offline support or
introducing rendered-artifact uploads. New proposals need a concrete problem
and evidence of a net reduction in cost or complexity.

The admission and capacity section that stood here is gone because nothing in
it is open. Comment admission was implemented, then dropped on 2026-09-23 in
favour of deleting the unread configuration keys, so a comment is not
refusable and accumulated comment storage per document is bounded only by the
general request bucket. Label budgets went the same way. Scan recovery and
the maintenance, reconnect and mixed-traffic capacity scenarios now live in
`storage::worker_recovery_tests` and `storage::postgres::benchmarks`; the
benchmarks have never been run, so measure before adding worker concurrency
or raising the pool. Retention's hourly catalogue read is documented for
operators in [docs/hosting.md](docs/hosting.md#resource-limits).

## The one open idea: companion and agent interface cleanup (original 2.9)

This area offers substantial possible deletion, but mixes obsolete code with
product choices. Companion presets, bindings and isolated workspaces still
serve local builds and projects with unshared resources, so their removal
needs a replacement contract rather than a deletion.

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

## Contracts to preserve throughout

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
