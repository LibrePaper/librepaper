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

Resource bound verification is done (§2.1) and focused browser session
lifecycle hardening is done (§2.2); companion and assistant cuts need a
clearer product decision.

| Priority by payoff | Idea | Why it matters | Recommended next step |
|---|---|---|---|
| 1 | Resource bound verification | Ensures the simplified controls still bound overload | Done 2026-09-20: background wake-ups, retries and deadlines bounded; retained limits inventoried and exercised |
| 2 | Browser session lifecycle hardening | Prevents stale async joins and callbacks from outliving their connection or session | Done 2026-09-20: cancellation gaps closed within the existing owners, with regressions |
| 3 | Companion and assistant scope | Potentially large cuts, but some remove useful entry points | Confirm unused paths; decide which local and agent workflows to support |

Suggested delivery order:

1. Done: background queue saturation, retry and deadline bookkeeping are
   bounded, and the retained admission limits are documented and exercised.
2. Done: async join and teardown boundaries were hardened within the existing
   browser session owners, preserving document and proposal contracts.
3. Remove confirmed obsolete companion paths independently; make larger product
   cuts only after choosing the supported workflow.

History truncation, narrower offline support and new compaction architectures
are conditional options, not prerequisites. The log cutover already collected
many of the original review's largest gains; do not count them again.

The original idea numbers are retained in parentheses below so references to
this review remain understandable.

## 2. Highest-value remaining work

### 2.1 Resource bound verification (remaining original 2.7)

**Done on 2026-09-20**, apart from the admission gaps recorded at the end of
this section, which are findings rather than remaining work on this task. The
inventory this section asked for is [docs/resource-bounds.md](docs/resource-bounds.md),
which documents every retained limiter and semaphore by the resource it
protects, its scope, its caller and what a caller that hits it observes.

**What was bounded.** The background worker's retry and deadline bookkeeping
was the one piece of overload state with no cap. `Worker::failed` grew a
`cooling` map with no ceiling and spawned one sleeping task per retry, and
`Worker::schedule_at` spawned one timer per call, so repeated observations of
one pending deletion accumulated timers. Both are now entries in a single
bounded map, `storage::schedule::Deadlines`, holding at most 1024 entries and
keyed by task identity. The worker sleeps on the earliest entry in its own
`select!`, so there is no spawned timer left anywhere in it. A map that is
full refuses the new deadline, keeps the earliest refused time, and wakes to
rescan durable state then, which is how refused work is found again.

Deduplication needed one correctness fix to work at all: `Task::Archive`
carried the document id beside the label, and the durable scan only ever
knows `Uuid::nil()` for that half, so one archive request was two different
task values that nothing could collapse. It now names the label alone.

**What was kept.** The bounded channel (256) and its single coalesced rescan
bit, bounded scan pages (64 per kind) with the continuation behind the page's
own work, the 60s backoff doubling to a one hour ceiling, the seven day
deletion grace, the restoration check in `claim_document_purge`, coverage
proof before any row is deleted, and every admission limit in sections 1 to 7
of the inventory. No new resource management framework was added and no
duplicate policy state came back.

**What an operator can now see.** `GET /api/status` gained a `background`
section: queue depth against its bound, deadline count against its bound, and
the two refusal counters. Neither refusal is an error (the work is handed
back to a durable rescan), but a deployment where they climb steadily is one
whose background work is behind, and until now that was invisible from
outside. A refused retry also logs at `warn` naming the task.

**Checks run** (2026-09-20, against PostgreSQL 16 in a throwaway container):

- `cargo nextest run --workspace`: 431 passed. `cargo fmt --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` are clean.
- 13 unit tests of the deadline map (`storage/schedule_tests.rs`): the bound,
  refusal and the earliest-refusal rescan, deduplication of repeated
  observations, that an ordinary delay never shortens a backoff, that a
  failure count survives being handed out and is reset by success, and that
  dead bookkeeping older than the retry ceiling is pruned.
- 6 PostgreSQL-gated end-to-end tests of the worker
  (`storage/worker_recovery_tests.rs`, `--test-threads=1`): 300 deletions
  asked for before the worker starts, saturating the channel, are every one
  of them purged; duplicate wake-ups purge once; a document inside its grace
  is not purged and one past it is; a deferred deletion runs when its grace
  ends with nobody asking again and no restart; a fresh worker finds durable
  rows with no wake-up at all; a restored document is not purged.
- The idle-query gate (`tests/deployment_gates.rs`) with a real worker,
  registry and sweeper running: zero statements in a 60 second window, and in
  the 600 second release window, counted exactly by `pg_stat_statements`.
  That branch of the gate had never run before (the extension is absent on an
  ordinary local server) and was measuring its own sampling query; the
  baseline is now taken after it rather than before.
- The authorization-revocation and lease-loss gates in the same file pass
  unchanged, so this did not delay revocation or durable flushes.

The grace and deadline tests fail against the implementation as first
written, which is how the one defect found in review was caught: a success
cleared the deadline the task had just armed for itself, which would have
left every deferred deletion waiting for a restart.

**Inspected rather than tested**, and worth saying so: a scan that fails
against an unreachable database retries on the same backoff, which is
exercised at the map level but not end to end, because taking the pool away
from a running worker leaves nothing to recover with. Work created *during* a
scan is covered by the rescan bit surviving a cursor continuation
(`overflow_rescan_waits_for_all_cursor_pages`) rather than by inserting a row
mid-scan against a real database.

**Admission gaps found while writing the inventory.** These are absences of
enforcement, not regressions from the cleanup, and each is a product decision
rather than a bug to patch quietly:

- `config.rate_per_hour` (comment rate, 20/hour) is defined and documented as
  "what one person may say about a document in an hour" and is read by
  nothing. Comments are bounded only by the general request bucket
  (6,000/minute). Both authenticated and commenter-link callers.
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
  catalogue read every hour whether or not anything expired. It is the one
  timer that breaks the idle-query rule, by design and only when configured.

**Still unverified:** that 20 PostgreSQL connections suffice under 64
concurrent HTTP work slots plus background work. That is a measurement, not
an inspection, and nothing here made it worse.

### 2.2 Browser session ownership (original 2.10)

**Assessment:** done as a focused lifecycle-hardening pass on 2026-09-20. The
evidence did not justify a broad browser architecture rewrite, and none was
made. `Reader.svelte` is unchanged apart from three guard clauses.

**Existing safeguards preserved:**

- `reader/collaboration.js` still owns the room, session, retry timer and
  session observers, with stale-response guards around reconnect metadata.
- `project-session.js` still owns local persistence status and durable
  coverage. Reader still derives save confirmation from that state; the
  durable-coverage reconnect fix from `efd8bb5f` is untouched, and neither an
  acknowledgement nor an empty send queue establishes durability.
- `reader/render-coordinator.js` still serializes and coalesces preview work;
  `reader/generation.js` still supplies the shared stale-work guard, and is now
  what `project-session.js` uses as well rather than a counter of its own.
- `Editor.svelte` still owns editor creation, subscription cleanup and
  destruction.
- `figures.js` caching was not touched.

**The gap, and what closed it:** `project-session.js` checked `left` after
hydration in `start()`, but not after fetching referenced state or verifying
its digest. A reproduction delayed that fetch, called `leave()`, then released
it: the session became `joined: true`, reported state and invoked its send
callback with `doc-update`. The same reproduction with `disconnected()` revived
the join and sent presence. No cross-document corruption or data loss was
demonstrated, and none is claimed.

The session now holds a connection generation (`reader/generation.js`), captured
when a join step is scheduled and rechecked after every asynchronous boundary in
`start()` and `rows()`: hydration, the referenced fetch, the digest, and both
error continuations, including the `restartBaseline` re-open. `disconnected()`
and `leave()` cancel that generation and also empty `joinGate`, so a fresh join
never queues behind a fetch nobody is waiting for; the abandoned step still runs
its continuations and finds itself obsolete before it touches `doc`, `send` or
`onState`. Ordering between frames that do still speak for the current socket is
unchanged, because they queue on the same gate as before. Local hydration is
deliberately captured even when the connection has gone, so an edit made offline
still counts as work waiting for durable coverage.

`report()` is suppressed after `leave()`, and `apply`, `applyPresence` and
`acknowledge` drop late frames rather than applying them to a destroyed
document. The persistence adapter is not cancelled with the session: an
outstanding write still completes, and only its delivery stops.
`reader/collaboration.js` gained a session generation of its own, so `onState`,
`onPeers`, `onSource`, `onSwap`, `onFiles` and `onAwareness` are not delivered
from a session that has been replaced or disposed. Reader guards
`onState`, `onPeers`, `onSwap` and `onAwareness` too, because teardown sets
`readerDisposed` before it closes the collaboration.

Lifecycle terminology is unchanged: Reader still captures its slug at
initialization and reloads on document identity or capability changes. No
in-place document navigation machinery was added.

**Regressions added:** `tests/unit/project-session.mjs` covers leave and
disconnect during the referenced fetch, an obsolete queued `doc-rows` frame, a
fresh join completing while the abandoned fetch is still in flight, late fetch
failures and late `restartBaseline` refusals, a digest that fails after the
connection has gone (and the same frame still rejecting on a live one), an
offline edit surviving a cancelled join, a persistence write landing after
`leave()`, and frames arriving after teardown. `tests/unit/reader-races.mjs`
covers a real session's late persistence notification after
`collaboration.close()` and an unguarded session that outlives its owner. Each
of the two reproductions was confirmed to fail against the unguarded code.

**Ownership boundaries** are now written down at the top of
`reader/collaboration.js`, including the teardown order.

**Sacrifice:** no user-visible functionality was given up.

**Done:** stale joins can no longer revive a disconnected or closed session,
send through a later connection or update disposed/replaced UI; fresh joins
still work; local recovery and durable save confirmation are intact. Run on
2026-09-20: `check:project-session` (project-session, collab-awareness,
reader-races), `check:render-coordinator`, the full `npm run check`
(113 passing), and the browser tests for split joins and buffered reconnect
(`doc-rows-browser.mjs`), offline reload (`offline-reload-browser.mjs`) and the
editor (`editor-browser.mjs`). The full browser suite has four failures
(`accessibility`, `menubar`, `responsive`, `shortcuts`) that reproduce on the
unmodified tree and are unrelated to this work.

Separate transport-level slow-subscriber recovery still deserves end-to-end
evidence, but is not proof that a browser ownership rewrite is needed.

### 2.3 Companion and assistant scope (original 2.9)

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
grows. The original review therefore proposed stage-level measurement before
changing storage. Those measurements now exist: the current
[log work notes](SPEC-server-is-a-log.md) report a cold build of a 1 MiB log at
about 8 ms and each other compaction stage under 35 ms, on one document in an
idle release process with PostgreSQL 16.15. They also report transient build
memory of 3 to 27 MB for logs of 1 to 3.5 MiB.

These results lower the priority of a compaction redesign; they do not establish
costs for every document shape or concurrent workload. Transient memory and
editing latency under load remain more useful questions than an assumed need
for a new storage architecture.

**If measurements justify more work, proceed in this order:**

1. Measure reconstruction, export, coverage verification, compression, upload
   and database activation separately, including editing latency and peak
   memory. Record failed/unreadable cases rather than reporting only successes.
2. Isolate export of a fixed durable prefix from live editing where lock
   contention is demonstrated. Budget the independent document and bounded
   blocking work; cloning a Loro handle does not isolate its state.
3. Tune compaction triggers to tail/base proportions and a recovery-time bound
   if small tails cause excessive full rewrites. Preserve startup scheduling
   and safe coordination between base activation, joining readers and deletion.
4. Consider immutable history segments with less frequent loading snapshots
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
