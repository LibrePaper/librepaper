# Document mutation paths

*2026-09-14. Inventory of production paths that can change a LibrePaper
document or a durable view of it. Classification is by who has authority over
the change, not by which data structure an implementation happens to use.*

## The three classes

### Local CRDT transaction

A peer changes its local Yjs/yrs document. The change becomes authoritative
only after the server admits and durably acknowledges the resulting update.
These operations must remain responsive and may be performed while temporarily
offline.

### Authoritative command

The server checks current identity, permissions, limits, and conflicting state
before committing a change. A command may internally create a CRDT update or a
snapshot, but its defining property is that a client cannot commit it by
writing arbitrary CRDT state.

### Snapshot transformation

A pure or isolated operation reads an immutable tree and produces another tree
or an output bundle. It does not mutate the live document. Applying its result
to the live document is a separate authoritative command.

This separation prevents a recurring category error: sharing an algorithm is
not the same as sharing authority.

## Inventory

### Local CRDT transactions

| User operation | Entry point | Mutation implementation | Server admission |
| --- | --- | --- | --- |
| Type, delete, paste, undo | `Editor.svelte`, `y-codemirror.next` | CodeMirror transaction against a `Y.Text` | `Room::receive_update` |
| Create a text file | `Reader.svelte::addFile`, file panel | `collab.js::addText` | `Room::receive_update` |
| Create an empty folder | file panel | `collab.js::addFolder` | `Room::receive_update` |
| Rename or move files/folders | file panel | `collab.js::relocate` in one Yjs transaction | `Room::receive_update` |
| Duplicate a file or asset | file panel | `collab.js::duplicateEntry` | `Room::receive_update` |
| Delete files/folders/assets | file panel | `collab.js::removeEntries` | `Room::receive_update` |
| Choose the main file | file panel | `collab.js::setMain` | `Room::receive_update` |
| Name an uploaded asset in the project | `Reader.svelte::addFigure` | blob upload followed by `collab.js::putAsset` | Blob route authorizes bytes; `Room::receive_update` admits the reference |
| Record a live tracked change | `Editor.svelte`, `track-changes.js` | text and revision metadata in one Yjs transaction | Revision transition validation in `room/revisions.rs`, then `Room::receive_update` |
| Synchronize filesystem edits | `cli/sync.rs` | `session::apply_edits`, `put_text`, `rename_path`, `remove_path`, `put_asset`, `remove_asset` on local yrs state | Sent as a Y update to `Room::receive_update` |

All browser directory mutations are grouped in `web/src/lib/collab.js`. All
native peer mutations above use `document/session.rs`. Their encoded document
shape is a compatibility boundary; neither side is an authority boundary.

`Room::receive_update` is the single admission path for peer-authored source
updates. It validates revision transitions and authorship, enforces visible and
encoded-size limits, repairs structural metadata, applies the update, marks the
session dirty, relays it, and persists before acknowledging it.

### Authoritative commands

| Command | Entry point | Live mutation | Durable effects |
| --- | --- | --- | --- |
| Add/reply/resolve/delete/refine/anchor a comment | room WebSocket or `POST /comments` | `Command` → `Room::apply_command_with_actor` | Annotation row and room cache |
| Accept or reject a legacy suggestion | room WebSocket or comments API | Accept applies guarded source edits; reject changes review state only | Annotation outcome plus checkpoint when source changes |
| Accept, reject, or undo a live tracked revision | `revision-decide` WebSocket command | `Room::decide_revision`; rejecting a pending change or undoing a rejection can edit `Y.Text` | Atomic session/revision write serialized with publication changes, then broadcast |
| Apply an agent/automation patch | document MCP/agent route | `Room::apply_agent_request` → `session::apply_path_edits` inside `checked_edit` | Session write, optional accepted suggestion, checkpoint, receipt |
| Apply a batch of assistant suggestions | `POST /suggestions` | Creates review records; does not change source | Durable annotations and checkpoint provenance |
| Restore a historical version | `POST /restore` | `Room::restore_and_checkpoint` → `session::restore` | Session write, update broadcast, restore checkpoint |
| Replace/republish source | `POST /documents` for an existing document | `Server::edit_into_session` constructs a wanted tree, then `session::restore` | Assets, session write, checkpoint, title, broadcast |
| Request a checkpoint | `y-checkpoint` | No source mutation | Immutable source version and manifest entry |
| Upload asset bytes | document asset route | No CRDT mutation until a peer names the digest | Authorized content-addressed blob |
| Delete a document | `POST /delete` | Removes the live room from service | Catalogue and stored roots are retired by storage policy |
| Change sharing or ownership | share/transfer routes | No source mutation | Authoritative catalogue permissions/ownership |
| Prepare and activate a publication | publication routes | No live-source mutation | Immutable display objects and atomic current-publication pointer |

HTTP and WebSocket comment commands deliberately converge at
`Message::into_command` and `Room::apply_command_with_actor`. They are two
transports, not two mutation implementations.

Restore and whole-project source replacement deliberately converge at
`session::restore`. Their authorization, asset ingestion, rollback, and
checkpoint policy differ, but reconciliation of a wanted tree with stable text
identities is one implementation.

### Snapshot transformations

| Transformation | Input | Output | Where used |
| --- | --- | --- | --- |
| Capture live tree | room-locked yrs document | `history::Tree` plus text bodies | Checkpoint, snapshot API, restore/republish rollback |
| Normalize historical tree | stored manifest/archive | normalized `history::Tree` | History and restore |
| Construct republish candidate | live tree plus uploaded directory | wanted tree and bodies | `Server::edit_into_session`, before authoritative restore |
| Validate/apply agent patches | immutable `SourceTree` plus expected passages | candidate `SourceTree` or conflict | Preview/validation before `Room::apply_agent_request` commits edits |
| Capture browser render tree | local Yjs session | plain texts, asset digests, main path | Preview, download, assistant candidate preview, publication |
| Render source | captured tree plus render options | HTML/PDF/DOCX, diagnostics and provenance | Browser renderer or local companion |
| Build display bundle | rendered HTML plus captured assets | self-contained publication HTML and objects | Publication prepare/activate |
| Compute history comparison | two immutable source trees | file and semantic differences | History UI and redlines; never changes live source |
| Assemble project download | captured tree plus fetched assets | ZIP/blob | Browser download only |

A snapshot transformation must not silently acquire permission or persistence
responsibilities. For example, `apply_patches` may prove that a candidate is
valid, but only `Room::apply_agent_request` may recheck current state and commit
it.

## Rules and where they belong

| Rule | Canonical owner | Other checks |
| --- | --- | --- |
| CRDT map names and value shapes | `document/session.rs` and `collab.js` compatibility surface | Cross-runtime interoperability tests |
| Path syntax and allowed extensions | Server `document/paths.rs` | Browser `paths.js`/`file-manager.js` provides early feedback; server remains authoritative |
| File identity survives rename/restore | `document/session.rs::restore` and CRDT schema | Browser operations preserve the same schema locally |
| Peer update admission and repair | `Room::receive_update` | Clients may prevent obvious invalid actions for usability |
| Revision authorship and legal transitions | `room/revisions.rs` | Browser controls are presentation only |
| Annotation validation and permissions | `room/command.rs` and `room/comments.rs` | Client optimistic state is recoverable, not authoritative |
| Source-edit conflicts | `wasm_helpers::text` edits plus guarded room operations | Browser previews may predict the result |
| Source snapshot identity | immutable source store/checkpoint code | Browser tree digests identify transient render inputs |
| Publication identity and activation | publication store and server publication gate | Browser builds and uploads the proposed bundle |

## Duplication decisions

### Already consolidated

- CLI sync, restore, republish, agent edits, suggestion acceptance, and tracked
  revision decisions ultimately mutate yrs documents through
  `document/session.rs`.
- WebSocket and HTTP annotation operations share the same parsed `Command` and
  room application path.
- Restore and republish share stable-identity tree reconciliation through
  `session::restore`.
- Rust text edits across agent and review flows use the shared
  `wasm_helpers::text::Edit` representation and UTF-16 coordinate convention.

### Similar by necessity; do not consolidate into one runtime

- `collab.js` and `document/session.rs` both know the Yjs document schema.
  Browser Yjs integration and native yrs admission need native access to their
  respective CRDT implementations. Keep compatibility fixtures; do not add a
  wasm boundary merely to remove these small schema adapters.
- Browser path validation duplicates server-visible policy so invalid file
  operations fail before creating a CRDT update. It is not an authorization
  check. The server must continue validating hostile or obsolete clients.
- Browser `tree()` and Rust `tree_of` both project CRDT state into plain trees.
  They run in different hosts and feed different local consumers. Their output
  contract should be fixture-tested, not routed through a network or wasm call.

### Candidate duplication requiring evidence

- Browser render trees, source checkpoint trees, agent `SourceTree`, Quarto
  result bundles, local builder responses, and publication display bundles use
  overlapping terms. Inventory their fields before introducing another common
  package. Consolidate vocabulary only where lifecycle and trust semantics are
  identical.
- Legacy suggestions and live tracked revisions both represent proposed source
  changes but have different authoring and conflict behavior. Do not merge
  their storage models until one can be removed without degrading live editing.

## Result

This audit does not justify a new editing engine or an immediate production
code consolidation. The important mutation implementations are already shared
within each trust boundary. The remaining cross-runtime similarities are
compatibility contracts, and hiding them behind wasm or server round trips
would add machinery without deleting the native integrations.

The next simplification target is the family of snapshot and build-result
shapes. It has plausible semantic duplication and can be investigated without
changing live editing, authorization, or recovery.
