# Document mutation paths

*2026-09-14, revised 2026-09-15 for the Loro cutover, revised 2026-09-19 for
the "server is a log" cutover (`SPEC-server-is-a-log.md`). Inventory of
production paths that can change a LibrePaper document or a durable view of
it. Classification is by who has authority over the change, not by which
data structure an implementation happens to use.*

## The three classes

### Ingest and flush

A peer changes its local Loro document and sends the resulting update. The
server decodes only its header, checks it is causally complete against the
log, appends it to the per-document sequencer's buffer, and relays it to
other editors immediately. The buffer becomes durable later, in one row, on
a flush (`SPEC-server-is-a-log.md` §5). The server never inspects or
validates the update's content on this path. These operations must remain
responsive and may be performed while temporarily offline.

### Semantic command

The server checks current identity, permissions, limits, and conflicting
state before committing a change. A command may internally prepare a source
update against the head cache, but its defining property is that a client
cannot commit it by writing arbitrary CRDT state: the command runs inside the
sequencer, evaluated against the cache and committed in one fenced
transaction (`SPEC-server-is-a-log.md` §7).

### Background work

A worker task derives durable artifacts from what ingest and flush already
made durable: a compacted base, an archive, a deletion. It does not append
semantic source edits and it holds no live document lock while it runs
(`SPEC-server-is-a-log.md` §8.6).

This separation prevents a recurring category error: sharing an algorithm is
not the same as sharing authority.

## Inventory

### Ingest and flush

| User operation | Entry point | Mutation implementation | Server admission |
| --- | --- | --- | --- |
| Type, delete, paste, undo | `Editor.svelte`, `vendor/loro-codemirror` | CodeMirror transaction against a `LoroText` | `Sequencer::ingest`, header check and gap check only |
| Create a text file | `Reader.svelte::addFile`, file panel | `collab.js::addText` | `Sequencer::ingest` |
| Create an empty folder | file panel | `collab.js::addFolder` | `Sequencer::ingest` |
| Rename or move files/folders | file panel | `collab.js::relocate` in one Loro commit | `Sequencer::ingest` |
| Duplicate a file or asset | file panel | `collab.js::duplicateEntry` | `Sequencer::ingest` |
| Delete files/folders/assets | file panel | `collab.js::removeEntries` | `Sequencer::ingest` |
| Choose the main file | file panel | `collab.js::setMain` | `Sequencer::ingest` |
| Name an uploaded asset in the project | `Reader.svelte::addFigure` | blob upload followed by `collab.js::putAsset` | Blob route authorizes bytes; `Sequencer::ingest` admits the reference |
| Record a tracked change | `Editor.svelte`, `proposals.js` | text written to the author's proposal branch, flushed on the next flush trigger | `proposal-update`, then the branch is stored beside the sequencer's log |

All browser directory mutations are grouped in `web/src/lib/collab.js`. All
native peer mutations above use `crate::log::Sequencer`. Their encoded
document shape is a compatibility boundary; neither side is an authority
boundary.

`Sequencer::ingest` is the single admission path for peer-authored source
updates. It authorizes the socket's role, bounds the update's size and rate,
decodes its header, refuses a causal gap with `doc-gap` rather than
attempting to repair one, appends the accepted batch to the buffer, relays it
to other editors, and opportunistically imports it into a warm cache entry.
It does not repair structural metadata: a shape the schema does not use is
absent from the projection with a diagnostic, not corrected
(`SPEC-server-is-a-log.md` §4.4). Durability is a separate step, `Sequencer::flush`,
triggered by age, quiet period, byte volume, a semantic command, the last
subscriber leaving, or shutdown (`SPEC-server-is-a-log.md` §5 step
6).

### Semantic commands

| Command | Entry point | Live effect | Durable effects |
| --- | --- | --- | --- |
| Add/reply/resolve/delete/refine/anchor a comment | room WebSocket or `POST /comments` | `Command` evaluated against the head cache, `Room::command` | Annotation row, evidence as head vector/frontier/digest. No source revision: a comment records the vector it was anchored at |
| Accept or decline a proposal hunk | `proposal-decide` WebSocket command | Branch merged and declined hunks reverted, prepared against the head cache | Decision row, resulting flush row if the merge changes source, and broadcast, in one transaction |
| Apply an agent/automation patch | document MCP/agent route | `Room::command` with an agent-patch `Command`; each expected passage is checked against head | Flush row, optional accepted suggestion, label, retry record |
| Apply a batch of assistant suggestions | `POST /suggestions` | Opens proposal branches; does not change the live document | Proposal rows and frontier provenance |
| Restore a historical version | `POST /restore` | `Room::command` with a restore `Command`; `expected_frontier` must equal the head frontier | Flush row, update broadcast, restore label |
| Replace/republish source | `POST /documents` for an existing document | Whole-project replacement `Command` | Assets, flush row, label, title, broadcast |
| Request a label | `doc-label` | No precondition; records head vector, frontier and digest | `document_labels` row |
| Upload asset bytes | document asset route | No CRDT mutation until a peer or command names the digest | Authorized content-addressed blob |
| Delete a document | `POST /delete` | Removes the live sequencer from service | Catalogue row and stored blobs are retired by storage policy |
| Change sharing or ownership | share/transfer routes | No source mutation | Authoritative catalogue permissions/ownership |

HTTP and WebSocket comment commands deliberately converge at `Room::command`.
They are two transports, not two mutation implementations.

Restore and whole-project source replacement deliberately converge on the
same `Command` machinery: fork the cache entry at head, apply the edit,
export the delta, and commit it with before/after evidence in one
transaction (`SPEC-server-is-a-log.md` §7.3). Their authorization, asset
ingestion, and label policy differ, but preparing a source-producing command
is one implementation.

There is no server-side repair pass and no repair command. Directory
collisions that valid concurrent operations produce are resolved by the
deterministic read rule in the projection algorithm, computed fresh on every
read, never written back.

### Background work

| Task | Trigger | Input | Output |
| --- | --- | --- | --- |
| Compaction | `uncompacted_update_count >= 100` or `uncompacted_update_bytes >= 16 MiB` after a flush | A cache entry whose `oplog_vv()` equals the log vector at the moment of the flush that crossed the threshold | A verified base blob; rows at or below the compacted sequence deleted in one fenced transaction |
| Archive materialization | A label's `archive_requested_at` is set and `archive_key` is not | `Project(cut)` at the label's frontier | An immutable archive blob, keyed by tree digest, referenced from the label row |
| Deletion | `documents.status = 'deleting'` | The document's rows, base, assets and labels | Storage reclaimed; catalogue row retired |

These run on the in-process worker described in `SPEC-server-is-a-log.md`
§8.6: a bounded queue with per-task backoff, reseeded at startup from durable
state (not a queue table), and otherwise silent while the deployment is
idle. There is no distributed job queue, no polling loop, and no
`maintenance_cursors` table. Compaction proves coverage by decoding the
snapshot's header and checking it against the log vector before it deletes
anything; a mismatch aborts and leaves the rows untouched
(`SPEC-server-is-a-log.md` §8.4).

## Rules and where they belong

| Rule | Canonical owner | Other checks |
| --- | --- | --- |
| CRDT map names and value shapes | `crate::log` and `collab.js` compatibility surface | Cross-runtime interoperability tests |
| Path syntax and allowed extensions | `librepaper-document-core::paths` | Browser `paths.js`/`file-manager.js` provides early feedback; server remains authoritative |
| Causal completeness of the log | `Sequencer::ingest`, the gap check | Clients export from the vector a `doc-gap` names; nothing is repaired after admission |
| The projection: paths, main file, digest | `librepaper-document-core::project`, `SPEC-server-is-a-log.md` §4.4 | Browser projection, held equal by shared fixtures |
| Proposal authorship and decisions | `room/proposals.rs` | Browser controls are presentation only |
| Annotation validation and permissions | `room/command.rs` and `room/comments.rs` | Client optimistic state is recoverable, not authoritative |
| Source-edit conflicts | `wasm_helpers::text` edits plus the sequencer's `Command` machinery | Browser previews may predict the result |
| Head identity | version vector, frontier and tree digest, evaluated fresh per command | Browser tree digests identify transient render inputs |

## Duplication decisions

### Already consolidated

- Restore, browser replacement, agent edits, and proposal decisions ultimately
  prepare a source delta through the same `Command::evaluate` /
  `Head::prepare` path and commit it through the same fenced flush
  transaction.
- Every change awaiting an accept or reject is one mechanism: a proposal
  branch. The separate revision, suggestion, agent pending-record and
  suggestion-column paths were removed rather than bypassed.
- WebSocket and HTTP annotation operations share the same parsed `Command` and
  `Room::command` application path.
- Restore and republish share the same `Command` preparation: fork the cache
  entry, apply the edit, export the delta.
- Rust text edits across agent and review flows use the shared
  `wasm_helpers::text::Edit` representation and UTF-16 coordinate convention.

### Similar by necessity; do not consolidate into one runtime

- `collab.js` and `crate::log` both know the Loro document schema. Both reach
  the same Loro core -- the browser through wasm, the server through the Rust
  crate -- so the schema is agreed on rather than reimplemented, but each
  host still names the maps for itself. Keep the fixtures that pin the
  names; do not add a wasm boundary merely to remove these small adapters.
- Browser path validation duplicates server-visible policy so invalid file
  operations fail before creating a CRDT update. It is not an authorization
  check. The server must continue validating hostile or obsolete clients.
- Browser and Rust both implement the projection algorithm of
  `SPEC-server-is-a-log.md` §4.4. They run in different hosts and feed
  different local consumers. Their output contract is fixture-tested
  (`web/tests/fixtures/projection.json`), not routed through a network or
  wasm call.

### Candidate duplication requiring evidence

- Browser render trees, proposal preview state, and archive export inputs use
  overlapping terms. Inventory their fields before introducing another common
  package. Consolidate vocabulary only where lifecycle and trust semantics
  are identical.

## What this removes from the previous inventory

The job queue, receipts, revisions, versions, bundles and server-side repair
that the earlier revision of this document inventoried are gone, per
`SPEC-server-is-a-log.md` §2.3 and §12:

- No `document_command_receipts` table and no per-keystroke receipt: only
  source-producing semantic commands carry a retry record, and it is a
  `document_labels` row.
- No `document_versions`, `document_source_revisions`, `bundles` or
  `bundle_files` tables, and no rendered-bundle publish/prepare/activate
  flow. Readers see head and refetch the projection.
- No `jobs` or `maintenance_cursors` table and no distributed job queue;
  background work is the in-process worker above.
- No server-side repair: `session::repair` and `session::restore`'s
  corrective path no longer exist. A shape the schema does not use produces
  a diagnostic, not a correction.

## Result

This audit does not justify a new editing engine or an immediate production
code consolidation. The important mutation implementations are already
shared within each trust boundary. The remaining cross-runtime similarities
are compatibility contracts, and hiding them behind wasm or server round
trips would add machinery without deleting the native integrations.

The next simplification target is the family of render-tree and archive
input shapes. It has plausible semantic duplication and can be investigated
without changing live editing, authorization, or recovery.
