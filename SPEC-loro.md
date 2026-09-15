# Loro collaboration substrate

*2026-09-15. Adopted implementation specification; not a claim about completed
work.*

## Purpose

Replace Yjs on both sides of LibrePaper with Loro: one Rust core, linked
natively into the server and compiled to WebAssembly for the browser. The
substrate change is the enabling half. The product change is that a proposed
edit becomes a branch, and the four mechanisms that implement "a change
awaiting accept or reject" today become one.

This specification complements
[REVIEW-architecture.md](REVIEW-architecture.md) and preserves its distinction
between local CRDT edits, authoritative commands, and snapshot transformations.
It supersedes the collaboration-engine assumptions in
[SPEC-offline-continuation.md](SPEC-offline-continuation.md) §"Current
foundations"; the product promises there are retained and §2.7 below
strengthens one of them.

## 1. Why

**1.1 One implementation instead of two.** `yjs` and `yrs` are independent
codebases agreeing on a binary format by convention and test coverage. The
drift surface sits on security-sensitive paths: `StickyIndex` encoded by `yjs`
and decoded by `yrs` months later to decide which range a rejection rewrites
(`room/revisions.rs:422`); a scratch-document rehearsal that assumes `yrs`
applies an update byte-identically to how `yjs` will (`room/revisions.rs:233`);
size bounds that depend on tombstone behavior matching. LibrePaper already uses
the single-implementation pattern deliberately — `wasm-helpers` is linked into
both the server and the wasm renderers so the word diff cannot drift. The CRDT
is the one place implementing the same format twice.

**1.2 Four proposal mechanisms become one.** "A change someone has proposed,
awaiting accept or reject" is implemented four times today:

| path | representation | size |
|---|---|---|
| tracked edits | JSON records with encoded anchors in the Yjs `revisions` map | `room/revisions.rs` 666 + `web/src/lib/track-changes.js` 446 |
| comment suggestions | a comment with `motivation: "editing"` and a `proposed` string (`room/comments.rs:148`) | `room/suggestions.rs` 175 |
| agent proposals | pending records written into the shared document | `room/agent.rs` 795 |
| branching | absent | — |

The first two are one feature with two entry points. The third exists because
an agent has nowhere to put work in progress except the document everyone
shares — which is why the server must police field-level mutations on every
inbound update. Under a branch model all four are one object: a fork plus a
merge decision, differing only in who opened it and how it is presented.

**1.3 The rehearsal disappears.** Because review state moves out of the CRDT
(§3.4), the server no longer needs `client_update_safe_after_as_with_len`
(`room/mod.rs:1757`) — a full document encode, scratch-document seed, and
double JSON parse on every admitted update, under the room state lock. This is
the current hot path and it is removed rather than optimized.

**1.4 Storage is neutral to favorable; history becomes free.** See §7.

## 2. Product contract

1. Local typing does not wait for network access or server acknowledgement.
2. Routine concurrent text edits converge automatically. Convergence is not a
   guarantee that concurrent prose edits preserve both writers' intentions.
3. A proposed change — typed with tracking on, offered from a comment, or made
   by an agent — is the same kind of object, reviewable the same way.
4. Accepting part of a proposal attributes the accepted text to whoever wrote
   it, and the removal to whoever declined it. Attribution is never silently
   reassigned.
5. A proposal's status is server-owned. A client cannot forge an accepted or
   rejected decision.
6. Every past state of a document is reachable. Version history is not limited
   to the checkpoints someone happened to take.
7. Reconnection after arbitrary absence merges without loss. There is no
   retention window past which an offline client's edits become unmergeable.
8. Source archives remain the format-independent record. Nothing in this
   specification makes document recovery depend on Loro.

## 3. Design

### 3.1 Document shape

One `LoroDoc` per document, with root containers mirroring the current layout
(`document/session.rs:30`):

| container | type | contents |
|---|---|---|
| `files` | `LoroMap` | file id → `LoroText` |
| `paths` | `LoroMap` | file id → path string |
| `assets` | `LoroMap` | path → digest |
| `meta` | `LoroMap` | `main`, `engine`, markers |

`revisions` is **not** carried over. Proposals are branches (§3.3) and their
status lives in Postgres (§3.4).

Renames stay identity-preserving: a rename moves a string in `paths` and leaves
the `LoroText` alone. `LoroTree` is deliberately not adopted in this phase —
it would collapse `files`+`paths` into one container, but that is a second
migration and is out of scope.

### 3.2 Text indexing is UTF-16, almost everywhere

Loro indexes text in Unicode code points by default. Every offset in
`session.rs`, in `wasm_helpers::text::Edit`, and in the browser counts UTF-16
code units, matching JavaScript. **All text access uses the `*_utf16` family**
— `insert_utf16`, `delete_utf16`, `splice_utf16`, `slice_utf16`, `len_utf16`,
`mark_utf16` — with one exception, named below, and no conversion at call
sites. A lint or review checklist enforces this; a single code-point-indexed
call is a corruption bug past the first astral character.

**The exception is `diff`, and it is not optional.** `LoroDoc::diff` returns
deltas indexed by the `wasm` feature of the crate that computed them: code
points with the feature off, UTF-16 with it on. There is no `diff_utf16`. The
server therefore describes an edit in code points and the browser describes the
same edit in UTF-16, and the two disagree past the first astral character —
which is precisely the corruption this section exists to forbid, sitting on the
newest path in the system.

This is contained rather than fixed, because a delta run is internally
consistent: its retains and deletes share one basis, so computing a diff and
applying it back through `apply_diff` on the same side is exact. Two rules keep
it contained, and both are asserted in `document::hunks`:

1. **A delta run never crosses the wire.** Review is presented by sending text
   and hunk boundaries, not by shipping a `DiffBatch` to the other side.
2. **A diff offset is never compared with a UTF-16 offset.** Hunk offsets are
   for ordering and display within one diff. Anything stored outside the
   document — a comment anchor, a review row — uses a `Cursor` (§3.7), which
   carries no integer basis at all.

`document::hunks::tests::diff_offsets_are_code_points_on_this_side_of_the_wire`
pins the server's basis, so a change in Loro's answer fails a test rather than
corrupting a paper.

### 3.3 A proposal is a branch

A proposal is a `LoroDoc` forked from the room document at a frontier:

```
main:  ──●──●──●──●──●──────────────●  merge
           └──○──○──○──○───────────┘
              branch: ordinary edits, no protocol
```

- **Tracked edit** — a branch opened as you type, usually one hunk.
- **Comment suggestion** — a branch opened from a thread, one hunk.
- **Agent run** — a branch with many hunks.

Review is `diff(base, tip)`. There is no tracked-change encoding, no anchor to
forge, and no status field inside the document.

**Hunks.** A hunk is a maximal run of non-retain deltas in the text diff: a
`Delete` and the `Insert` that replaces it are one decision, not two. This
grouping is ours; Loro has no hunk concept.

One refinement, found by building it. The diff arrives character-level —
replacing "cat" with "tabby" is not one delete and one insert but several tiny
edits with a letter or two retained between them, because that genuinely is the
shortest way from one to the other. Nobody reviews at that grain. So two
changes separated by **eight or fewer retained units** are one hunk: about a
word, on the reasoning that a change resuming within a word is the same edit as
far as a reader is concerned.

That number is a presentation choice, and it is load-bearing in a way a
presentation choice usually is not: a decision names a hunk by its index
(§5.2), so the browser and the server must group identically or "decline the
second one" reverts something else. Both sides carry the constant, and neither
may change it alone.


### 3.4 Accept, decline, and attribution

Loro's `apply_diff` re-authors applied text as the applying peer. Applying a
filtered diff therefore produces correct text with wrong authorship. **Partial
accept is implemented as merge-then-revert instead**, which is both correct and
semantically honest — the author wrote all of it, the reviewer removed part:

1. Import the whole branch into the room document. All of the branch author's
   operations, and their authorship, enter the graph.
2. Compute the inverse diff `diff(tip, base)`.
3. Filter it to the declined hunks only.
4. Apply that as the reviewer.

Verified: final text is exactly the accepted subset, and the version vector
retains both peers — the accepted text stays attributed to its author, the
removal to the reviewer.

Steps 1–4 are **one atomic server operation**. The intermediate state contains
the declined content, and must never be relayed to peers or persisted as a
distinct version.

Whole-branch accept is step 1 alone. Whole-branch decline is dropping the
branch without importing it.

**Review state** — proposal id, branch base and tip frontiers, author, status
(`pending`/`accepted`/`declined`/`superseded`), decider, decision time, and
comment — lives in Postgres, written only by the server command path. This is
what satisfies contract 5 and what lets §1.3's rehearsal be deleted.

### 3.5 Branch storage and lifecycle

A branch is stored as a Loro update blob (its operations since the fork),
alongside its Postgres row. It is not a separate room and holds no live
sockets. Because history is never trimmed (§3.6), a branch cannot be orphaned
by compaction and needs no expiry policy for correctness — only for tidiness.

### 3.6 History policy: keep everything

**No thinning. No shallow snapshots. No retention window.** Measured (§7.2),
full history for a realistically typed paper costs about the size of the paper,
and a thinned snapshot is consistently *larger* than the full operation log,
because character-level typing run-length-encodes almost perfectly while a
materialized state does not.

This decision removes, rather than solves, four problems: stale-client recovery
paths, branch expiry as a correctness requirement, the retention frontier
negotiation between offline support and time travel, and the one-way loss of
blame behind a trim point. It is what makes contracts 6 and 7 unconditional.

An escape valve remains for pathological documents: if a document's history
exceeds `max_encoded_snapshot_bytes`, it is refused at admission exactly as
today. Thinning is not the remedy; the remedy is the existing ceiling.

### 3.7 Anchors

`StickyIndex` becomes `Cursor`. Comments and any position stored outside the
document use `text.get_cursor(pos, side)` and resolve with
`doc.get_cursor_pos`. A `Cursor` carries its container ID, so the guard in
`offsets_of_sticky_indices` — that a resolved anchor lands in the file it names
— becomes a container comparison rather than a `BranchPtr` comparison.

### 3.8 Presence

`y-protocols` awareness, including the hand-rolled Rust encoder at
`cli/peer.rs:559`, is replaced by Loro's `EphemeralStore` on both sides.

## 4. Persistence

The existing shape is retained: one compressed base plus an ordered Postgres
update log, with periodic compaction (`storage/collaboration.rs:1`).

- **Base** — `ExportMode::updates` from an empty version vector: the full
  operation history, and the smallest full-history representation Loro offers.
  Not `ExportMode::Snapshot`, which additionally stores a materialized state
  and is roughly twice the size (§7.1).
- **Compression** — zstd-3, unchanged.
- **Blob key** — `documents/{id}/collaboration/{base_id}.loro.zst`. The `.yrs`
  suffix distinguishes pre-migration bases and must not be reused.
- **Update log** — `document_updates` unchanged; rows hold Loro update bytes.
- **Source archives** — unchanged, and remain the format-independent record
  (contract 8).

## 5. Wire protocol

Nothing has shipped, so there is no compatibility window to protect and no
reason to carry Yjs's names on Loro's payloads. The `y-*` prefix goes in the
same change as the substrate: `doc-open`, `doc-sync`, `doc-state`,
`doc-update`, `doc-update-start`/`-chunk`/`-end`, `doc-ack`, `doc-presence`,
`doc-peers`, `doc-checkpoint`. `doc-presence` carries `EphemeralStore` bytes.

This reverses the original plan of renaming later. That plan existed to avoid
debugging a rename and a substrate change together, which is sound when the two
are separable — but a hard cutover makes them one change anyway, and leaving
`y-` on the wire would be exactly the kind of vestige §6 now forbids.

### 5.1 Proposal messages

The review interaction is modelled on Google Docs' suggesting mode, which fixes
the message shapes:

| message | direction | carries |
|---|---|---|
| `proposal-open` | client → server | the branch's base frontier |
| `proposal-update` | client → server | branch update bytes |
| `proposal-list` | server → client | open proposals, each with author, base and tip |
| `proposal-decide` | client → server | proposal id, hunk index, accept or decline, and the tip the decision was computed against |
| `proposal-decided` | server → clients | the outcome, and the room update it produced |

Three properties follow from the Google Docs model, and each one costs
something:

**Decisions stream; they are not batched.** Clicking accept records that
decision and broadcasts it at once. There is no review session to submit, so
`proposal-decide` names one hunk, not a set. When the decision reaches the
document is a separate question, answered in §5.1a.

**The author keeps typing while a proposal is open.** A proposal is therefore
never frozen, and a decision can race an edit to the same branch. This is why
`proposal-decide` carries the tip it was computed against: a decision against a
stale tip is refused and the client recomputes, rather than accepting text that
has since changed underneath the reviewer.

**Main keeps moving too.** A proposal's base goes stale by design, which is the
case `document::hunks`' stale-base test covers.

### 5.1a When a decision reaches the document

A decision is **recorded** the moment it is made and broadcast to everyone
reviewing. The document changes when the proposal **resolves** — when every
hunk has been decided, or the reviewer closes it with the rest declined.

This is not the obvious design and it is worth saying why it is the right one.
Merge-then-revert (§3.4) has to know which hunks were declined *before* it
merges, because the revert is computed against the branch tip. Applying each
decision to the document as it arrives would mean merging on the first accept
and then, on a later accept, re-applying a hunk that had already been
reverted — as the reviewer, since the reviewer is the peer doing the applying.
That is exactly the re-authoring §3.4 exists to avoid: the author's prose would
end up attributed to whoever approved it, which is contract 4 broken in the one
place it was written to protect.

The alternative — merge everything up front and revert only what is declined —
keeps attribution but puts un-accepted prose into the shared document, where a
client that does not know about proposals, or a render, or an export, would
show it as though it had been agreed. A proposal that is indistinguishable from
the document is not a proposal.

So the merge happens once, atomically, when the proposal resolves, exactly as
§3.4 describes. What streams is the decision, not the text.

For a tracked edit this distinction is invisible: it is usually one hunk, so
deciding it resolves the proposal and the document changes on the click. It is
visible only on a multi-hunk proposal — an agent run, say — where the paper
changes once at the end rather than in pieces. That is also the better
behaviour there: a run that touches many files should land or not land, rather
than leaving the paper half-rewritten while somebody works through the list.

### 5.2 Diffs are computed on both sides, and never exchanged

Suggestions render inline in the editor, so the browser needs hunk boundaries
in its own UTF-16 basis, while the server needs them in its own to apply a
decision. Per §3.2 the two bases disagree, and there is no conversion.

Each side therefore computes the diff it needs from the document state it
already has. A `DiffBatch`, or any delta run, never crosses the wire in either
direction. `proposal-decide` names a hunk by **index within the diff of a named
tip against a named base** — three identifiers, no offsets — which is the one
way to name a hunk that means the same thing on both sides.

### 5.3 What branching makes possible, which suggesting mode does not

*Provisional. §5.1 is a floor, not a ceiling; this section records what the
substrate opens up so the protocol does not accidentally close it.*

Google Docs' suggesting mode is a flat overlay: every suggestion is an
annotation on one linear document. That is why two people rewriting the same
sentence produces a tangle rather than a choice, and why there is no way to ask
what the document would read like with some suggestions taken and others not.
Neither limitation is essential. Both come from not having real branches.

**Contending proposals are alternatives, not a collision.** Two branches that
touch the same passage are not a merge conflict to be resolved — they are two
answers to the same question, and the useful presentation is side by side with
one choice to make. This is the normal case in a co-authored paper: two people
who disagree about a sentence, or a coauthor and an agent who have rewritten
the same paragraph. The server can identify contention without being told, by
testing whether two proposals' hunks overlap on their common base, so this is a
presentation decision rather than a protocol one.

**A speculative reading of the paper.** Because a branch is a fork and not an
annotation, any *subset* of open proposals can be merged into a scratch
document and read as finished prose — "show me the paper with Alice's
introduction and Bob's conclusion" — without committing to either. Reviewing a
paragraph in isolation is how you accept a sentence that ruins the paragraph
after it; reading the result is how you catch that. This is the one capability
here that needs a message, so §5.1 gains:

| message | direction | carries |
|---|---|---|
| `proposal-preview` | client → server | a set of proposal ids and the tip of each |
| `proposal-preview-state` | server → client | the document text as those proposals would leave it |

It returns text, not a document and not a delta: it is a reading, nothing syncs
to it, and nothing is persisted. A preview that returned a `DiffBatch` would
violate §5.2.

**Blame that survives review.** Contract 4 keeps attribution correct through a
partial accept and §3.6 keeps every intermediate state reachable. Together
those answer a question no tool answers well today: after a chain of proposals,
each partly accepted, who actually wrote the sentence that is in the paper now?
That is not an extra feature to build so much as something the substrate stops
throwing away — the work is in asking for it, not in keeping it.

None of this blocks Phase 2. What Phase 2 must not do is bake in the assumption
that a proposal is an overlay on one document: the hunk identity in §5.2 —
proposal, base, tip, index — is deliberately independent of any particular
rendering, and `document::hunks` has no notion of where a hunk is displayed.

## 6. Implementation phases

Each phase has an exit gate. A failed gate stops work at that phase rather than
proceeding.

### Phase 0 — Review interaction prototype — **done**

Built against throwaway Loro documents and landed as `document::hunks`: hunk
grouping, whole-branch merge, and merge-then-revert partial accept, with the
branch-base-has-moved-on case covered. §11 records the results, including the
`diff` indexing defect the prototype found.

*Gate: passed.*

### Phase 1 — Server substrate

This runs first. The original ordering put the editor binding ahead of the
server, but Phase 2's gate asks that remote edits behave as today, and only a
server that speaks Loro can make that true.

Port the Rust. `document/session.rs` has 35 public functions and is the core of
the work; `room/mod.rs`, `cli/peer.rs` and `tools/fuzz` follow. `yrs` leaves
the dependency list in this phase, not later.

- `encode_state` → `export(updates from empty VV)`; `encode_vector` →
  `oplog_vv`; `encode_diff` → `export(updates from vv)`.
- `decode_update`'s `catch_unwind` is removed: Loro's `import` returns
  `LoroResult` rather than panicking. Malformed input must still be refused,
  not partially imported.
- `admit_decoded_update`'s size and file-count bounds are recomputed against
  Loro encodings. The `yrs`-era constants are not carried over untested.
- `edit_candidate`'s scratch-document pattern is replaced by `fork`.
- Presence moves to `EphemeralStore` (§3.8), and the hand-rolled `y-protocols`
  encoder at `cli/peer.rs:559` is deleted.
- The wire messages are renamed off the `y-` prefix (§5).
- The Rust crate and the npm wasm build are pinned to one version from one
  place, and CI fails if they diverge. `tools/check-loro-pin.sh` does this.

*Gate:* the fuzz target passes, now asserting one implementation's round-trip
rather than cross-implementation agreement; the full suite passes; `yrs` does
not appear in `Cargo.toml`.

### Phase 2 — Editor binding

Replace `yCollab` with `loro-codemirror` in `components/Editor.svelte`, and
write the IndexedDB persistence that replaces `y-indexeddb` — no such package
exists for Loro, and contracts 1 and 7 rest on it. `MergeEditor.svelte`
follows.

The current integration reaches into `ySyncFacet`, `ySyncAnnotation` and
`yUndoManagerKeymap`, including a Vim `u` remap through `vimUndoKeymap[0].run`.
`loro-codemirror` is younger and has sat at 0.3.3 since October 2025, so some
of that reach has no equivalent.

*Gate:* cursors, remote edits, undo grouping and multi-file switching work, and
every behaviour the binding cannot support is listed and deliberately dropped.
A silently lost behaviour fails this gate; a named and accepted loss does not.
Offline edits survive a reload.

### Phase 3 — Proposal model

Implement branches end to end for **one** entry point — tracked edits.
No flag gates it: §10 rules out coexistence, so the old mechanism is deleted in
the same change. Postgres schema for review state, server command path for
accept and decline, branch storage, and the proposal wire messages of §5.1.

`web/src/lib/track-changes.js` is rewritten against it rather than ported: its
dependence on observers firing during transaction cleanup so their writes join
the same update has no Loro equivalent, and is replaced by explicit commit
boundaries.

The hunk identity that §5.2 fixes — proposal, base, tip, index — must not
acquire a dependency on how a hunk is rendered, so that §5.3 stays reachable.

*Gate:* tracked edits work end to end on Loro with review state in Postgres;
the rehearsal at `room/mod.rs:1757` is deleted rather than ported; and no delta
run crosses the wire in either direction.

### Phase 4 — Fold in the other proposal paths

Migrate comment suggestions (`room/suggestions.rs`) and agent proposals
(`room/agent.rs`) onto branches. Delete `room/revisions.rs` and the
`revisions` container. This is where §1.2's consolidation is realized; Phases
1 and 3 only make it possible.

*Gate:* one proposal mechanism remains. `revisions.rs`, `suggestions.rs` and
the agent's pending-record path are removed, not merely bypassed.

### Phase 5 — Existing documents are not carried across

LibrePaper is unreleased. That is the premise the hard cutover rests on, and it
settles this phase by removing it: `yrs` bases are dropped, the next open of a
document starts a fresh `LoroDoc`, and no migration code is written.

This is worth stating rather than leaving implicit, because a migration is the
single most expensive thing in the original plan and the only remaining reason
to keep `yrs` in the tree at all. Writing one would mean a `yrs` build-time
dependency, golden fixtures from production-shaped documents, anchor re-minting
across two CRDT libraries, and an idempotent resumable runner — all to serve
documents that no user has.

Source archives are untouched and remain the format-independent record
(contract 8). A document whose content genuinely matters is recoverable from
its archive, which is exactly the property §4 keeps archives for, and it is
recovered by importing the archive into a fresh document rather than by
decoding a `yrs` base.

*Gate:* `yrs` appears nowhere in the tree — not in `Cargo.toml`, not in a tool,
not behind a feature. `grep -r yrs` finds only prose.

## 7. Measurements

Reproduced by the harness described in §7.4. Sizes are zstd-3 bytes, the
compression `storage/collaboration.rs:97` applies.

### 7.1 Export mode selection

300 KB of prose, one author:

| | raw | zstd-3 | zstd-19 |
|---|---|---|---|
| plain text | 300,000 | 81,854 | 72,079 |
| `yrs` update-v1 | 300,024 | 81,903 | 72,126 |
| **loro updates (chosen)** | 300,259 | **82,187** | 72,348 |
| loro Snapshot | 330,531 | 170,960 | 161,385 |
| loro ShallowSnapshot | 159,500 | 124,499 | 119,654 |

The all-updates export is within 0.3% of `yrs` and of the plain text.
`ExportMode::Snapshot` is twice the size because it stores a materialized state
in addition to history; it is the wrong durable unit and §4 forbids it.

Raising zstd from 3 to 19 buys about 6% on every format equally, and a
dictionary trained on the same prose makes every format worse. Compression
settings are not a lever here.

### 7.2 History is cheap; thinning is counterproductive

Character-by-character typing, the way a paper is actually written:

| scenario | doc | keystrokes | full history | thinned |
|---|---|---|---|---|
| drafted only, 1 author | 100 KB | 100,000 | **29 KB** | 41 KB |
| lightly revised, 1 author | 100 KB | 146,316 | **43 KB** | 46 KB |
| heavily revised, 3 authors | 101 KB | 386,619 | **118 KB** | 248 KB |
| rewritten many times, 3 authors | 104 KB | 1,049,312 | **286 KB** | 605 KB |

A thinned snapshot is larger than the full operation log in every case.
Contiguous keystrokes by one peer run-length-encode to a single run; a
materialized state does not. A million keystrokes of history costs 286 KB,
against a 16 MB admission ceiling and a 32 MB compressed base limit. This is the measurement §3.6 rests on.

### 7.3 Rebuild costs

176 KB document, 2,000 commits:

| | |
|---|---|
| cold start, loro base | 0.65 ms |
| cold start, `yrs` base | 0.19 ms |
| checkout to an old version | 3.9 ms |
| diff two versions | 3.9 ms |
| source archive fetch | 0.08 ms |

Cold start is sub-millisecond either way. Archives remain the fastest
point-in-time retrieval and are retained for that reason (contract 8). What
changes is capability, not speed: `yrs` garbage-collects deleted content
(`skip_gc: false` is the default and `session::new_doc` does not override it),
so it **cannot** reconstruct a past state at all. Contract 6 is unavailable
today at any price.

### 7.4 Reproducing

The harness replays one scripted edit stream into both libraries over prose
drawn from `docs/`, and reports raw and compressed sizes per export mode. It
is not yet in the repository; landing it under `tools/` is tracked in §9.

## 8. Testing

- **Fuzz** — `tools/fuzz` retargets to Loro. The invariant weakens usefully:
  round-trip within one implementation, rather than agreement between two.
- **Attribution** — a test asserting that after merge-then-revert the accepted
  text's author appears in the version vector and the declined text's does not
  survive in the state. This is contract 4 and it has no equivalent today.
- **UTF-16** — property tests over astral characters through every text path,
  asserting browser and server agree on offsets (§3.2).
- **Migration** — golden documents from production-shaped fixtures, asserting
  §6 Phase 5's gate field by field.
- **Offline** — a client absent for longer than any previous retention window
  merges cleanly, asserting contract 7.

## 9. Follow-ups, deliberately out of scope

1. Land the measurement harness under `tools/` so §7 is reproducible.
2. Rename the `y-*` wire messages (§5).
3. Evaluate `LoroTree` to collapse `files` + `paths`.
4. Reconsider checkpoint density now that contract 6 makes every intermediate
   state reachable; archives may be needed less densely.
5. Delete `max_encoded_snapshot_bytes`'s speculative-encode machinery
   (`room/mod.rs:210-284`) if Loro's bounds make it unnecessary.

## 10. Decisions taken

*Closed 2026-09-15.*

**Branch blobs live in Postgres**, alongside review state. §3.4 requires
merge-then-revert to be one atomic operation; one transaction over one store is
how that is actually achieved. Symmetry with bases argued for the blob store,
but bases are large, long-lived and read on cold start, and branches are none of
those things.

**A tracked edit opens one long-lived branch per author per session.** The
argument for a branch per edit was that per-hunk decline would be easier. Phase
0 removed it: `document::hunks` declines one hunk of a two-hunk branch and keeps
attribution straight either way, so the cheaper object count wins on an even
footing.

**An agent run is one branch**, matching §3.3's "a branch with many hunks". A
run that touches many files is still one decision a person is making about one
piece of work.

**Pending revision records are not migrated**, because nothing is migrated
(Phase 5). This supersedes the earlier decision to carry them across as
branches, which was taken while a migration was still expected. Unreleased
means there is no unreviewed work to preserve.

**The cutover is hard, with no coexistence.** Nothing has shipped, so breaking
changes cost nothing and there is no reason to pay for a migration window.
`yrs`, `yjs`, `y-protocols`, `y-codemirror.next` and `y-indexeddb` are removed
in the same change that adds Loro. No feature flag gates the proposal model, no
adapter translates between the two encodings, and no code path survives to read
a `yrs` base outside the one-shot migration in Phase 5. This is what §6's phases
are now sequenced against, and it is why §5 renames the wire messages
immediately rather than sweeping later.

**Review starts from Google Docs' suggesting mode**, which settles the
interaction §5 was waiting on: suggestions render inline, decisions are per hunk
and apply on click, and both the author and the rest of the room keep editing
throughout. §5.1 draws out what that costs. It is a floor rather than a
settled design: §5.3 records what real branches make possible that a flat
overlay cannot, and Phase 2 is required to keep that door open even though it
need not walk through it.

**Browser offline persistence is ours to write.** `loro-indexeddb` does not
exist. Loro exports and imports update blobs, so the shim is small, and
contracts 1 and 7 are not worth regressing in the meantime.

**`loro-codemirror` is adopted as-is, and the editor gives up what it cannot
do.** Phase 1's gate is relaxed accordingly: rather than requiring that every
current behaviour survive without patching the binding, it requires that what
the binding cannot support is *deliberately dropped and named*, not silently
lost. The reach into `ySyncFacet`, `ySyncAnnotation` and `yUndoManagerKeymap` —
including the Vim `u` remap — is where that bill comes due.

## 11. Phase 0 results

*Recorded 2026-09-15 against `loro` 1.16.0 and `loro-crdt` 1.16.1.*

### 11.1 The gate passes

Merge-then-revert (§3.4) was exercised against throwaway documents: a two-hunk
branch, partial accept, whole accept, whole decline, and a branch whose base
had moved on. In every case the final text is exactly the accepted subset, and
the version vector retains the base author, the branch author and the reviewer
— so the accepted prose stays attributed to whoever wrote it and the removal
to whoever declined it. That is contract 4, and it now has a test.

The prototype is `document::hunks`, landed rather than thrown away: the hunk
grouping and the declined-hunk filter are the parts Phase 2 would otherwise
have written twice.

Every API the specification names exists: the full `*_utf16` family, `fork` and
`fork_at`, `diff` and `apply_diff`, `get_cursor` and `get_cursor_pos`,
`oplog_vv`, and `ExportMode::Updates`. `EphemeralStore` is re-exported from the
facade crate as `loro::awareness`, so §3.8 needs no dependency on
`loro-internal`. `DiffBatch`'s fields are private but `iter` and `push` are
public, which is all §3.4's filter step requires.

### 11.2 `diff` is not UTF-16 indexed

See the amendment to §3.2. This is the one finding that changes the design
rather than the plan.

### 11.3 Gaps in scope

Three surfaces this specification does not currently account for.

**Browser offline persistence.** `web/src/lib/collab.js:4` uses `y-indexeddb`
to hold the document across reloads and offline spells. There is no
`loro-indexeddb`; the package does not exist. Contracts 1 and 7, and the
promises retained from [SPEC-offline-continuation.md](SPEC-offline-continuation.md),
rest on this surface, and replacing it is unscoped work that belongs in Phase 1
rather than being discovered during it.

**`loro-codemirror` is the ecosystem's neglected binding.** It sits at 0.3.3,
last published 2025-10-07, while `loro-prosemirror` moved to 0.4.4 in August
2026. `Editor.svelte` reaches into `ySyncFacet` three times, `ySyncAnnotation`
once, and `yUndoManagerKeymap` four times, including a Vim `u` remap through
`vimUndoKeymap[0].run`. Phase 1's gate forbids patching the binding. That gate
is the likeliest one to fail, and it is cheap to test before anything else is
built on it.

**Phase 1 cannot precede Phase 3.** Phase 1's gate asks that remote edits and
cursors "behave as today", which requires a server that speaks Loro; Phase 3 is
what makes the server speak Loro. Either the server port moves ahead of the
editor binding, or Phase 1's gate is honestly reduced to a local-document
test — the binding's reach into internals, not its sync behaviour, is what that
phase is really measuring.
