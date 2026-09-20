# Big architectural ideas

*2026-09-19. Two independent architecture reviews of the working tree, merged.
Status: ideas for decision, not a specification and not a description of work
done. The brief was robustness, cost and simplicity, with explicit permission to
lose functionality and edge-case guarantees in exchange for large structural
gains.*

Starting point: 82K lines of Rust, 35K of browser code, 20K of browser tests,
989 commits in fifteen days, unreleased. At the time of these reviews, the
document-engine proposal was partly implemented in the uncommitted diff:
fenced single-writer transactions, receipts, a source-revision index,
commit-before-relay for typing, and a lease-based command owner per room.
That proposal has since been retired; [the log architecture](SPEC-server-is-a-log.md)
records the current design, surviving decisions, and outstanding work. The
review below preserves its original context; later additions are dated.

## 1. Shared thesis

Both reviews arrive at the same diagnosis from different directions. The
collaboration engine is being asked to be four things at once:

1. a live merge substrate for concurrent typing;
2. a permanent historical record of every intermediate state;
3. a precise, character-level authorship ledger for review;
4. an authoritative validator that repairs and measures hostile bytes.

Responsibilities 2, 3 and 4 generate most of the code and most of the database
work, and each of them is defensible only for edge cases. Keep responsibility 1.
Move the other three off the typing path, or drop them.

The spec on the table preserves all four (indefinite history, offline directory
editing, operation authorship through partial acceptance, server-side candidate
validation). Relaxing them opens simplifications an order of magnitude larger
than the spec attempts.

## 2. The ideas

Ordered roughly by structural payoff. Each states the sacrifice explicitly.

### 2.1 The server is a durable log, not a document engine

Today a live room holds a `LoroDoc`, a 14-field `Session`, a lease-based
command owner, a resident-size estimator, a 671-line repair pass for malformed
directories, and a fork, validate, commit, install cycle per typing batch. The
spec adds candidate preparation and a full encoded-history measurement per
batch. All of it defends against bytes sent by editors, who already have
unlimited write authority over the text. The threat that matters is resource
exhaustion, and byte quotas handle that without decoding anything.

- **Typing path.** Append update bytes to `document_updates` and relay them.
  No decode, no fork, no repair, no measurement. Limits are per-update bytes,
  per-document cumulative bytes, per-principal rate. This is how y-websocket,
  Automerge sync servers and Liveblocks work.
- **Everything document-shaped becomes a pure function of durable bytes.**
  Source projection for readers, comment anchoring at a frontier, proposal
  diffs, export, agent reads: load base plus updates, compute, cache by update
  sequence, discard under memory pressure. The room shrinks to a subscriber
  list, a sequence counter and a flush buffer. Eviction becomes trivial because
  nothing resident is unique.
- **Path normalization becomes a view, not a mutation.** Two files at one path
  resolve by a deterministic display rule on read. Both browsers compute the
  same answer from the same state. The repair module and "server corrections
  in the delta" disappear. (If 2.4 makes directory operations online commands,
  the collision cannot occur at all and even the view rule goes.)

Sacrifice: server-enforced document shape, server-side repair, and the
guarantee that a reused request id with changed content is refused. A malformed
update is one every client's Loro import rejects and skips; shared state stays
consistent.

Gain: roughly 4,000 lines of Rust, the hottest lock in the server, and every
per-batch cost that today scales with a document's accumulated past.

### 2.2 Proposals as patches, not CRDT branches

Partially accepting a proposal currently imports the whole branch and reverses
the rejected hunks, because applying only the accepted text would attribute it
to the reviewer rather than the author (`room/proposals.rs`). The browser and
server therefore each reconstruct identical branch hunks, synchronised by a
hand-maintained constant (`SAME_DECISION_WITHIN` in `proposals.js`, mirrored
in `document/hunks.rs`) with nothing but a comment enforcing agreement.

Store a proposal as a base revision plus replacements. On acceptance, verify
the affected source still matches, apply the accepted replacements atomically,
and record "suggested by Alice, accepted by Bob" as ordinary metadata.

Sacrifice: exact character-level provenance, automatic handling of some
concurrent edits, speculative merging of arbitrary proposal combinations. A
conflicting suggestion goes stale and needs refreshing.

Gain: suggestions, tracked edits, agent review and partial acceptance all
survive with a conventional implementation. Review decouples from the operation
graph. The `document_proposals.branch_bytes` column, the hunk-identity contract
across languages, and the server-side branch import path go. This composes
directly with 2.1: with no branches there is no server-side hunk computation
left to keep resident.

### 2.3 Bounded collaboration history, unbounded snapshots

Historical reconstruction, old proposals, cursors and offline merging all
depend on retaining the operation graph forever, and the native validator
exports the entire history to measure it. A small edit's cost depends on the
document's whole past.

Separate two promises:

- Saved versions remain readable and restorable as plain project snapshots.
- Automatic CRDT merging works within the current collaboration generation.

At an explicit maintenance boundary, start a fresh generation from the current
source. A client returning from an older generation keeps its local work and
is offered a recovery copy or a reviewed patch instead of a silent merge.
Comments keep their original quotation and snapshot reference; attachment
across generations is best effort with an explicit "unplaced" state.

Sacrifice: seamless merging after arbitrarily long absences, perpetual blame,
access to every intermediate keystroke.

Gain: bounded live state, bounded replay for the lazy projections in 2.1,
predictable validation cost, less permanent dependence on one CRDT encoding.
Much easier after 2.2, since old proposals are the main thing that pins old
history.

### 2.4 Narrower offline

There is a large gap between "do not lose my typing when the connection drops"
and "reopen a complete development environment offline, modify its directory,
and reconcile everything later". The implementation supports the broader
promise, including per-tab update journals and local compaction
(`offline-projects.js`), and the spec adds account-scoped caches, incarnation
checks and multi-tab compaction proofs.

Smaller contract:

- Preserve unsynced text locally; survive disconnection and reconnect.
- Require connectivity for creating, deleting and renaming files.
- Offer recovery or export when automatic reconciliation fails.
- Optionally one editing tab per project per browser profile.

Sacrifice: some offline workflows and simultaneous local tabs.

Gain: far fewer combinations of directory conflict, cache preparation, account
change and recovery. This pays off only if the offline machinery is actually
removed; adding an online command path beside the existing offline paths
would increase complexity.

### 2.5 Save latency: the one point where the reviews differ

Both agree on batching per document rather than per editor, a fixed policy
rather than adaptive batching, and immediate durability for comments and
decisions. They differ on typing.

- **Commit-before-relay, 2 to 10 second batches.** Simpler invariant: nothing
  visible is undurable. Collaborators see typing with a delay. Illustratively,
  1,000 continuously active documents produce about 500 source commits per
  second at 2 seconds and about 100 at 10 seconds. These are transaction
  counts, not measured capacity or prices.
- **Relay first, flush every 10 to 30 seconds of quiet, acknowledge on
  durability.** Cheaper and faster for collaborators, but it restores the
  distinction between visible and durable state that the current rewrite is
  removing, and it needs recovery machinery.

The resolution depends on 2.1. If the server keeps a resident document,
relay-first recreates a speculative room and commit-before-relay is the right
call. If the server becomes a log, the only undurable state is a byte buffer,
and its loss on crash is recovered by the protocol clients already implement
(local persistence plus version-vector resend). In that world relay-first costs
no extra machinery and is the better choice. Any semantic command flushes the
buffer in its own transaction first, so a comment never names source that is
not durable. Decide this after deciding 2.1, not before.

### 2.6 Idempotency by client-generated ids, not receipts

Comments already carry a `temp_id`. Make it the primary key and insert with
ON CONFLICT DO NOTHING. Hunk decisions are keyed by proposal and hunk. Proposal
opens take a client id. The lifetime `document_command_receipts` table goes and
every semantic operation costs one row instead of two.

Sacrifice: rejecting a reused id with different content.

### 2.7 Harvest the single-writer decision

One writing process per deployment is already accepted and implemented
(advisory lock plus epoch). The dividend has not been collected.

- **Delete the jobs table and the poller.** Compaction, archive
  materialization and deletion sweeps are derived work. Keep an in-process
  queue and re-derive pending work at startup by scanning counters. That
  removes `jobs.rs`, claim tokens, expired-claim recovery and dedupe indexes.
- **Kill every one-second timer.** The room sweep, the link reauthorizer, the
  per-socket tick and the worker poll each fire every second; the cost meter
  upserts every minute. Check link expiry on the next frame from that socket.
  Run eviction on a 60 second timer or on admission. Afterwards an idle
  deployment issues zero queries; only the lease connection stays open.
- **Delete the cost governor and two of the three rate limiters.**
  `server/cost.rs` is an egress budget, four-scope token buckets, four
  semaphores, six histograms, a Postgres checkpoint and a seven-timer pressure
  machine behind one mutex, beside `socket_budget.rs` and the per-room rate
  map. Keep one per-principal token bucket at ingress and one per-document
  byte quota. Egress protection belongs to the provider spending cap and the
  reverse proxy the hosting guide already requires.
- **Collapse the history schema.** Six tables describe history (updates,
  bases, source revisions, versions, checkpoints, activity) plus the dead
  bundle tables. A source revision is an update row. A checkpoint is a label on
  an update sequence. A plain-source archive is an export artifact computed on
  demand and cached in the object store by tree digest, not a durable table
  with a worker and a 202 state. Activity derives from update rows if they
  carry peer and time. Two tables remain: updates and labels. Sacrifice: the
  guarantee that a plain copy exists at checkpoint time before anyone asks.
- **Config.** Forty-five knobs, each restated four times (policy struct,
  override mirror, JSON limits report, CLI flag). Hard-code all but the handful
  an operator varies: origin, database, object store, storage quota, session
  key. Delete the override mirrors and the provenance map.

### 2.8 One document command boundary

The current `DocumentOwner` task grants leases while callers still operate
through a separate state mutex (`room/owner.rs`). Ordering is a convention
across about seventy `acquire()` sites; the mutex is reachable without the
lease. Pick one mechanism.

- Under 2.1 the owner's job shrinks to a flush buffer and fan-out, and a plain
  lock is enough.
- If the resident document stays, build a real actor that owns the state and
  executes typed commands, because batching and persistence then have one
  natural home. Do not keep both.

### 2.9 Cut whole products out of the product

- **One agent path, not four.** The remote MCP endpoint plus the stdio bridge
  already lets Claude Code, Codex and anything else edit a document. The
  sidebar assistant is a second product: an ACP runner, a durable task journal,
  a lifecycle supervisor, a server-side chat relay with three render maps, a
  companion launcher and about 1,500 lines of Svelte. Delete it, or at most
  make the sidebar a thin ACP client that talks to the companion on loopback,
  which removes the server relay. Replace the 2,092-line `agent_query` module,
  which serves one MCP tool, with source and search.
- **Companion diet, if it stays.** Protocol v1 is declared dead but fully
  implemented on both sides. LaTeX vestiges thread a `--tex-path` flag through
  six files to reach a discovery function that no longer looks for TeX, and a
  325-line TeX log parser exists for logs the companion cannot produce. Reduce
  builders to Quarto only (Typst renders in the browser already). Replace
  presets and grants with one approved command per bound folder. Drop live
  watched previews in favour of build-on-save through the same job path.
  Consider dropping frozen-cache verification (640 lines validating Quarto's
  own cache format) in favour of a binary per-document execution consent; an
  unapproved Quarto document falls back to the browser subset renderer that
  already exists. Zotero and agent launching are the only pieces without a
  browser substitute.
- **Device flow.** A personal API token copied from a settings page replaces
  the device-code flow, its token cache and the CLI polling side, about 1,500
  lines. Worse UX for a rare action.

### 2.10 Browser consolidation

The spec already says one session owner. The concrete cuts:

- seven interlocking state machines (socket, join, session, confirmation,
  persistence, presence, editor binding) down to three: connection, document,
  render;
- one render coordinator instead of the three the LaTeX stack adds;
- one cache keyed by content digest instead of two IndexedDB databases plus
  two Cache Storage namespaces, and one local project name instead of four;
- the truth moved out of `Reader.svelte` (about 60 `$state` and 80 `$derived`
  wired to twenty controllers through callback bundles) rather than more
  callbacks added.

### 2.11 A trade-off to name, not re-litigate

Removing publication moved rendering cost from the server to every reader. A
Typst reader downloads 10 MB of brotli-compressed wasm; a LaTeX reader
compiles against a third-party mirror that also sees every self-hosted
deployment's traffic. If that ever bites, the cheap middle is one rendered
artifact per document, overwritten by the editor's tab on quiet, with no
versions table. One object, one PUT.

### 2.12 The largest option: one active editor, no CRDT

If simultaneous editing itself is negotiable: one active editor per document,
revision-checked saves, everyone else submitting comments or patches. That
removes the CRDT, both Loro integrations, the sync stack, offline
reconciliation and most of sections 2.1 through 2.5 in one move. It is the
biggest reduction available and also the clearest change to what LibrePaper
is. Named here so the decision is explicit, not recommended by either review.

### 2.13 Make compaction affordable as history grows

*Added 2026-09-20, following review of the server-as-log implementation.*

Compaction currently exports a full-history Loro snapshot, imports it into a
second document to verify coverage, compresses it with zstd, and uploads it.
A cold cache also requires reconstructing the document first. This reduces
the number of stored update rows but retains the operation history: a small
new tail can trigger another rewrite of the document's entire past. The
dominant cost has not yet been measured.

Start with improvements that preserve the history and recovery contract:

- Measure reconstruction, snapshot export, verification, compression, upload
  and database activation separately, on representative long-lived documents
  in release builds. Measure editing latency during compaction as well as
  total compaction time.
- Compact a fixed durable prefix in a separate document so export does not
  hold the live sequencer lock. Budget the additional memory and CPU work;
  a cloned Loro handle is not an independent document. Coordinate base
  activation and deletion with readers so a join cannot miss covered rows.
- Move verification and compression onto bounded blocking workers. This
  protects request responsiveness; it does not itself reduce total CPU work.
  Keep coverage verification before deleting any source rows.
- Trigger compaction according to the new tail's size relative to the base,
  with a recovery-time limit, instead of repeatedly rewriting a large history
  after a small fixed number of rows. Ensure threshold-crossing writes and
  startup recovery actually schedule the work.

If repeated full-history rewrites still dominate, consider immutable history
segments plus less frequent snapshots for fast loading. Preserve the segments
needed for historical reads and offline reconciliation. This trades more
involved recovery, indexing and garbage collection for fewer rewrites; it is
an alternative to the simplicity of one base plus a tail.

The larger option connects to 2.3: use Loro shallow snapshots to bound active
history, with older history archived separately if it must remain readable.
This requires an explicit retention boundary and recovery for offline peers
older than that boundary, plus a policy for old proposals and comment anchors.
It is not a transparent replacement for full snapshots. See
[Loro's shallow snapshot documentation](https://loro.dev/docs/concepts/shallow_snapshots).

Sacrifice: additional background memory and scheduling complexity for isolated
compaction; more storage machinery for segmented history; or seamless merging
across arbitrary absences if active history is truncated.

Gain: responsive editing during compaction and less repeated work as a document
ages. Start with measurements and lock isolation before changing history
retention; no speedup estimate is justified yet.

## 3. What it adds up to

Approximate lines removed if the recommended (non-2.12) set is adopted:

| Area | Lines |
|---|---:|
| Live room engine, repair, candidate cycle, resident accounting | 4,000 |
| Branch-based proposals, hunk identity across languages | 1,500 |
| Cost governor, socket budget, second and third rate limiters | 1,700 |
| Jobs queue, worker, archive job, bundle remnants, receipts | 3,000 |
| Sidebar assistant stack, chat relay, agent query | 9,000 |
| Companion v1, LaTeX vestiges, presets, previews, non-Quarto builders, frozen checks | 4,500 |
| Config mirrors, device flow | 2,000 |
| Offline breadth (per-tab journals, multi-tab compaction, cache migration) | 500 |

Roughly a quarter of the Rust and a tenth of the browser code. The database
write budget becomes at most two rows per active minute per document under
relay-first (or one row per batch interval under commit-before-relay), one row
per comment or decision, and zero queries when idle.

## 4. Order and decision points

1. **Decide 2.1** (server as log). It determines 2.5, 2.8 and whether the
   spec's remaining stages are worth finishing.
2. **Do 2.2 and 2.6** next. Patch proposals and client-id idempotency are
   independent of the typing path and remove cross-language coupling.
3. **Then 2.7**: timers, jobs table, cost governor, history schema, config.
   Independent, fast, and the whole of the idle-database cost.
4. **Then 2.3 and 2.4**: bounded generations and narrower offline. These are
   product decisions with migrations; they get easier once 2.2 has landed.
5. **2.9 and 2.10** can run any time; they do not touch the document path.

Keep throughout: transactional acceptance for semantic commands, honest save
indicators, authorization at the commit boundary, document isolation across
origins, and the single-writer lease. None of the above requires weakening
those.
