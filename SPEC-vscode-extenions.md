# VS Code CRDT extension

*2026-09-14. Proposed product and implementation specification; not a claim
about current behavior.*

## Purpose

Make VS Code a supported collaborative source editor for LibrePaper. The
extension participates directly in the document CRDT while active, rather than
waiting for saved files and reconstructing every change from snapshots.

The product promise is responsive source editing, automatic synchronization of
routine concurrent edits, durable local continuation, and explicit recovery for
ambiguous or refused changes. It is not a promise that arbitrary modifications
made while the extension is inactive merge invisibly.

This specification follows [REVIEW-architecture.md](REVIEW-architecture.md) and
shares durability and authority requirements with
[SPEC-offline-continuation.md](SPEC-offline-continuation.md). It does not require
the browser offline implementation to ship first.

## Scope

The first release supports one VS Code installation connected to an existing
LibrePaper project, ordinary source files in a local folder, live collaboration
with browser peers, offline continuation after setup, and restart recovery.

Later increments may add preview, diagnostics, source navigation, presence,
comments, and tracked-change controls. Publishing, review decisions, sharing,
and history restoration remain authoritative server operations.

Arbitrary external editors, concurrent extension/CLI writers on the same
folder, first-time offline checkout, and full offline review workflows are not
initial promises. Remote extension hosts and multi-root workspaces require
explicit qualification before being advertised as supported.

## User workflow

1. **Open LibrePaper project** authenticates to a chosen server and downloads an
   existing project into an empty folder. Existing nonempty folders require an
   explicit import/comparison workflow; never overwrite them during attachment.
2. The extension opens ordinary project files. Local buffer changes become CRDT
   operations immediately, including before the user saves the file.
3. Remote operations update the corresponding buffers without disrupting local
   selection or generating duplicate local transactions.
4. Connection loss changes sync status but does not disable local writing.
   The extension durably records CRDT state independently of file saves.
5. On restart it loads the local session before attempting the network, checks
   actual buffers/files against the recorded projection, and preserves any
   divergent content before reconciliation.
6. On reconnect it validates identity and permissions, exchanges CRDT updates,
   and reports synced only after durable server acknowledgement.

The attach workflow explains once that buffer edits are shared live: **Save**
controls the disk file, not whether collaborators see an edit. Dirty-buffer
status and LibrePaper synchronization status remain distinct.

## Architecture and ownership

Use the existing encoded document schema and room admission path. Yjs is the
initial client implementation to evaluate in the extension host; the Rust server
retains yrs and authoritative validation. Do not introduce a new editing engine
or a snapshot-upload path for each save.

Separate three owners:

- **Project session:** owns CRDT state, identity, persistence, transport,
  acknowledgements, reconnect, and recovery. It has no VS Code UI dependencies.
- **Editor adapter:** binds CRDT file identities to VS Code documents, translates
  buffer transactions, maps selections, and implements collaborative undo.
- **Workspace adapter:** projects source and assets to ordinary files, observes
  external disk changes, handles paths, and detects competing local writers.

Extract transport-neutral/client-session code from the browser only where its
semantics actually match. Inject persistence and transport instead of importing
DOM, IndexedDB, reader components, or browser authentication assumptions into
the extension host. Share compatibility fixtures even where runtime adapters
remain separate.

The local companion remains a build worker with an explicit request/result
contract. It need not own synchronization or become a mandatory daemon. A
separate service is justified only by measured lifecycle or multi-client needs.

## Buffer transaction contract

Stable CRDT file IDs identify text; paths are mutable names. Use the existing
UTF-16 coordinate convention and test its translation against VS Code ranges.

The adapter must:

- Apply multi-change events against the correct document version, with defined
  ordering for disjoint edits. Never use offsets from a newer buffer against an
  older CRDT projection.
- Distinguish local typing, remote application, recovery, formatting, and undo
  origins. Remote application must not echo back as newly authored text.
- Serialize or rebase remote buffer application across intervening local edits.
  An asynchronously applied edit must not overwrite input received meanwhile.
- Map cursor and selection positions through remote operations.
- Handle Unicode, line endings, multiple cursors, paste, composition input,
  formatter edits, and save participants.
- Preserve text identity through rename/move and keep the main-file identity
  consistent with the shared project.

Collaborative undo is a release gate: undoing a local action must not remove
another author's intervening work. Redo must remain coherent after remote edits.
Do not assume VS Code's ordinary undo stack automatically provides these
semantics. Prototype and document the supported API strategy before committing
to the integration. If ordinary file-backed documents cannot meet this contract,
revisit the editor surface and product scope rather than silently shipping
destructive undo.

## Disk and CRDT relationship

While attached, the project session owns collaborative source state. Disk files
are a usable projection, and unsaved buffers may legitimately be newer than
disk. Saving does not replace the CRDT with the whole file.

Persist a hidden, versioned sidecar containing server/document incarnation,
CRDT state or journal, confirmation bookkeeping, stable file mappings, and the
last known disk projection needed to recognize external changes. Reuse or evolve
the existing CLI baseline contract where semantics match; do not create two
competing baselines for the same folder. Credentials belong in VS Code secret
storage, not the project sidecar.

Use crash-safe persistence and compaction. On restart compare the sidecar,
current disk files, and any restored dirty buffers before applying remote state.
Back up divergent content before a potentially destructive reconciliation.
Neither disk nor a restored editor buffer automatically wins every case.

Changes made while the extension was inactive are snapshot reconciliation against
a baseline, not recovered CRDT operations. Unambiguous text changes can be
translated into operations; ambiguous delete/edit, rename, or whole-file
replacement cases need a concrete comparison and recovery action. Missing or
corrupt sidecars trigger recovery/reattachment, not a guessed full replacement
of the remote project.

Detect another extension host or `librepaper sync` writing the same project.
Enforce a single workspace adapter through a shared locking/ownership mechanism,
or stop attachment with an actionable explanation. File-watcher feedback loops
must not become duplicate edits. Handle atomic editor saves and exclude sidecar,
credential, generated-output, and unrelated files from synchronization.

Initial asset handling downloads existing assets for local use. New or changed
asset bytes require online upload and acknowledgement before publishing their
CRDT references. Offline asset edits remain local with explicit pending status;
they must not create references that other peers cannot resolve.

## Synchronization and authority

Use the existing room protocol's update and receipt mechanisms. Preserve CRDT
identity across restart and reconcile conservatively after a lost receipt.
Verify protocol compatibility, update-size handling, and schema version before
editing remotely. Publish any necessary protocol extensions as explicit shared
contracts, not extension-specific bypasses.

All peer source updates pass through `Room::receive_update`. Local permission
checks provide feedback but do not replace current server authorization.
Reconnection checks server identity and document incarnation before replay;
the same slug on another server or a recreated document is a different target.

Expired credentials permit recovery of already local work, not remote access.
Permission refusal, rejected updates, quota failures, and deleted projects leave
local changes exportable. Never repeatedly send a terminally refused update in
an endless reconnect loop or display it as synced.

Status distinguishes local session persistence, file-save state, and remote
confirmation. A successful file save alone does not establish that collaboration
metadata survived a crash; a successful socket send does not establish server
durability.

## Review and preview boundaries

The initial adapter must preserve existing review metadata and stable identities
when editing source. It must not fabricate tracked-change attribution or silently
claim that ordinary VS Code edits were recorded as tracked revisions. Qualify
behavior when browser peers are using tracked changes before releasing mixed
client collaboration.

Later review UI uses authoritative commands for review decisions, retaining
expected state and conflict checks. Source anchors that no longer resolve are
shown as unresolved. Rich review integration depends on the separate continuity
design in the architecture review.

Preview and diagnostics consume an immutable source snapshot, preferably of
current buffers, with explicit source/configuration identity. Reuse companion
build results and provenance contracts. Results from old builds must not be
shown as diagnostics for newer source. Executing project build tools requires
the appropriate workspace trust and existing folder-consent boundary.

## Delivery and go/no-go gates

1. **Adapter feasibility:** one text file, browser plus VS Code, concurrent
   typing, selection mapping, formatting, local undo/redo, and remote edits
   racing with local input. Decide whether ordinary buffers meet the contract.
2. **Durable session:** local persistence, offline restart, lost receipts,
   identity checks, rejection recovery, and truthful status.
3. **Project support:** multi-file identity, rename/delete, assets, crash recovery,
   external disk reconciliation, and extension/CLI ownership exclusion.
4. **Writing trial:** complete a real multi-user writing session with outages
   and restarts. Record intervention frequency, confusing merges, and recovery
   success before making the seamless-collaboration promise.
5. **Optional integrations:** preview/diagnostics, then review features whose
   continuity and command semantics have been specified and tested.

Do not begin broad review UI or multiple editor integrations before the first
two gates pass. If buffer synchronization or collaborative undo cannot be made
reliable, retain the explicit file import/comparison workflow as the fallback.

## Required verification

Use shared Yjs/yrs fixtures and session tests plus real VS Code integration
tests; a mock editor alone cannot validate event ordering or undo behavior.

- Concurrent inserts/deletes, non-BMP characters, mixed line endings, multi-cursor
  edits, IME input, format-on-save, and code actions.
- Remote application racing with typing, save, undo, redo, close, and reopen.
- A local undo after remote edits preserves the remote contribution.
- Crash before/after local persistence and server commit, including a lost ack;
  recovery does not duplicate operations or overwrite restored dirty buffers.
- Browser and extension converge after editing offline on both sides; retained
  versions permit recovery from semantically confusing merges.
- Rename with an open dirty buffer, concurrent rename, delete/edit, main-file
  change, and remote restore retain identities or expose unresolved outcomes.
- External whole-file edits, atomic saves, corrupt/missing baseline, two VS Code
  windows, and a competing CLI sync process cannot silently corrupt the project.
- Authentication expiry, revocation, recreated documents, server rejection, and
  local disk-full failures preserve work and produce truthful status.

Document the tested VS Code versions and host environments at implementation
time. This specification does not assume unverified extension API capabilities.
