# The collaborative editor is the architecture

*2026-09-13. Architectural directions after reviewing the code and clarifying
the product priority. These are proposals to investigate, not an approved
implementation plan.*

## Product premise

LibrePaper is primarily a collaborative editor. Publishing, review, history,
AI assistance, and local compilation should grow out of that editor. The
README's emphasis on publishing and collecting comments understates this
priority.

The central question is: **how do we build one excellent editing engine that
the browser UI, AI agents, CLI tools, history, and review all operate through?**

Implementation churn is acceptable. Simplification should preserve or improve
the editing experience. The collaborative editor remains the core product.

**Comments and tracked changes must carry forward to the current version.**
They cannot be confined to the publication or snapshot where they originated,
and carrying them forward is not an optional import step. Preserve their
identity, authorship, discussion, and review history as the document evolves.
When a target is deleted or cannot be mapped confidently, keep the record
visible in the current review workflow with an explicit conflict or unresolved
anchor, rather than dropping it or guessing a new target.

## What the current architecture tells us

The document model spans browser JavaScript and server Rust.
`document/session.rs` exposes the server representation, while
`web/src/lib/collab.js` and `web/src/lib/track-changes.js` work directly with
Yjs. The browser also uses `y-codemirror.next`. Sharing the implementation
would require editor integration work, not just packaging Rust as wasm.

The server has legitimate document responsibilities. `Room::receive_update`
validates revisions, enforces document and encoded snapshot limits, repairs
structure, and applies updates. `Room::decide_revision` can change text as
well as review state. Making the server ignorant of documents is not itself
a useful objective.

Useful boundaries already exist: immutable source versions in
`storage/source.rs`, publication staging and activation in
`storage/publication.rs`, and durable collaboration recovery in
`storage/collaboration.rs`.

The earlier application line count included CLI and local tooling. It was
not a server size measurement. Architecture changes need evidence about
coupling, correctness, and development cost rather than a target line count.

## Candidate directions

### 1. One editing engine, several entry points

Define project-changing operations once: creating and renaming files,
applying edits, accepting proposals, restoring content, and capturing
snapshots. Browser UI, AI agents, and CLI tools should use the same operation
semantics and conflict rules.

This is a shared contract and ownership boundary first. It does not require
a network request for every keystroke: interactive typing stays local and
responsive, with the CRDT coordinating concurrent edits. Authoritative
operations still enforce permissions and validate the state they change.

A shared Rust document crate, compiled natively for the server and to wasm
for the browser, is one implementation candidate. Requiring every host to
execute identical wasm bytes needs its own justification. Existing Yjs editor
bindings are a major part of any prototype.

**Potential gain:** fewer implementations of editing rules and fewer special
mutation paths for automation, review, and restore.

**Tradeoffs:** preserve editor transactions, undo, selection, and responsiveness.
Shared code still needs compatibility tests for old browser sessions,
persisted documents, and host bindings. It reduces implementation drift;
it does not eliminate version skew.

### 2. Keep the CRDT focused on live editing

Keep collaborative editing central, using the existing Yjs/yrs family rather
than inventing a CRDT. Give the editing engine responsibility for producing
ordinary project snapshots containing files and assets.

Publishing, export, and durable version history should consume snapshots
wherever they do not need live collaboration semantics. The CRDT update log
remains necessary for recovering acknowledged edits between snapshots.
A source snapshot alone cannot replace that recovery log.

Snapshots also cannot sever review continuity. Any boundary around the CRDT
must preserve the identities and mappings needed to carry comments and tracked
changes into the current document, including after restore or replacement of
source files.

Permissions and authoritative review decisions need explicit ownership
outside arbitrary client mutations. Moving decisions to Postgres is a
candidate, but does not delete all validation: the current validator also
protects authorship, immutable proposal fields, anchors, and deletion rights.
Accepting, rejecting, or undoing a revision may modify live text. A migration
must define how that change and the database decision commit, retry, recover,
and appear together to clients.

**Potential gain:** fewer features depend on CRDT internals while retaining
the collaborative editor.

**Tradeoffs:** snapshot boundaries, revision identity, recovery, and consistency
between stores must be explicit. The server still understands documents where
correctness requires it.

### 3. Explore proposed patches as a common editing operation

A proposal could identify a base version, an expected passage, and its
replacement. Acceptance checks the current document and either applies the
change or presents a conflict. Rejecting an unapplied proposal never requires
reversing text that somebody else has subsequently changed.

Browser suggestions, AI edits, and reviewer changes could share this
representation and acceptance path. Prototype it inside the editor, with an
inline preview and a comparison when the target has changed.

A proposal's base version supplies provenance, not a place to leave it behind.
Pending proposals and existing tracked changes must remain available against
the current version, with explicit conflicts when their targets change.

**Potential gain:** one proposal mechanism across several editing workflows,
with clearer conflict behavior.

**Tradeoffs:** this is not an automatic replacement for tracked changes.
Live tracked changes may provide the better authoring experience. Test typing,
overlapping suggestions, undo, and collaborative review before deciding which
workflows should use patches. Familiar behavior should change only if the
editing experience improves.

### 4. Make review continuity part of the document model

Comments and tracked changes follow the evolving document. Collaborators can
edit and review the live draft without publishing first. External reviewers
may read a fixed publication, but their comments must also enter the current
version's review workflow, retaining the publication and passage they saw.

Use stable review-record identities and explicit mappings between original
targets and current targets. Ordinary edits, file moves, restores, source
replacement, and any future branch merges must preserve that continuity.
When mapping fails, retain the comment or tracked change and expose the
unresolved target for attention. Carrying a tracked change forward does not
mean automatically accepting it or applying its text to a guessed location.

Historical publications can remain immutable views of what was reviewed.
They cannot be the only home of a review record. A static reader may avoid
loading a live editing session, but review storage and mapping still need to
connect its annotations to the current document.

**Potential gain:** one continuous review workflow across editing and
publishing, with provenance explaining both the original and current context.

**Tradeoffs:** this retains substantial coupling between editing and review.
Identity, anchor mapping, and tracked-change semantics need an explicit design;
frozen publications do not remove that work. Quote selectors can be ambiguous
when passages repeat. Client-side resolution is useful for display, but
authoritative text edits still need checks against current state, as
`revisions::guarded_inverse` does today.

### 5. One publication package across renderers

Have Markdown, Typst, LaTeX, and Quarto builders produce a common package:
rendered output, assets, diagnostics, and optional mappings back to source.
The package identifies the source snapshot it was built from. The reader
and publication storage consume that contract.

Source mappings should be optional capabilities. Without them, a reader may
annotate output but cannot automatically apply a source edit. The contract
must accommodate HTML and PDF without pretending their navigation and
selection models are identical.

**Potential gain:** fewer format-specific branches through publication and
review, and a clearer route for improving or adding renderers.

**Tradeoffs:** preserve source navigation, figure annotations, incremental
preview, and format-specific capabilities. A common interface should not force
every keystroke to produce and upload a complete publication bundle.

### 6. Make the local companion a build worker with a narrow contract

A project snapshot and build request go in; a publication package and
diagnostics come out. Tool discovery, subprocess management, and platform
details live behind that boundary. Browser builders can satisfy the same
logical contract, despite different execution and permissions.

Folder access, pairing, cancellation, and incremental builds remain explicit
capabilities. Preserve the interactive preview loop. A future remote worker
could implement this contract, but is not necessary to justify it.

Extract `local` into a separate workspace crate with a deliberate dependency
boundary. A separate binary may follow; a separate repository is not required.
Adding a binary that still links the entire application library would not
deliver the intended build isolation.

**Potential gain:** independent development and testing of native tooling,
with less platform-specific coupling in the editor and server.

**Tradeoffs:** protocol evolution and shared dependencies still need management.
Measure compile/link time separately from test execution. The current timeout
configuration identifies scheduling under load as a source of flakes; crate
extraction cannot promise to fix those failures.

## Supporting investigations

### Quota and admission

Distinguish incoming wire bytes, visible project size, encoded CRDT size,
and stored assets. They measure different things. Cheap synchronous bounds
may reduce work; some product limits might tolerate eventual enforcement.

Do not defer a limit without defining what happens when already accepted
edits exceed it. Persistence and resource limits need stronger guarantees
than a billing allowance might. Moving quota checks alone does not remove
document parsing from the update path: revision validation, structural repair,
and encoded snapshot admission also inspect document state.

### Distribution

Keep distributed rooms deferred until capacity or availability requirements
justify them. A durable log already supports collaboration recovery; it does
not by itself make room ownership, fanout, or authoritative commands stateless.
A future distribution design must address fencing, replay, compaction,
delivery, and decision ordering. Replacing a mutex is not sufficient.

## What to explore first

1. **Extract local tooling into a workspace crate.** Measure the build loop
   before and after, and use the extraction to clarify dependencies.
2. **Specify the editing engine's operation boundary.** Follow representative
   browser, AI, CLI, review, and restore operations through the current code.
   Identify which rules can actually have one implementation.
3. **Prototype the common build/publication contract.** Exercise a browser
   builder and a native builder, including preview and source navigation.
4. **Prototype proposals inside the editor.** Compare the experience with live
   tracked changes before choosing a shared representation or migration.
5. **Design snapshot and review ownership together.** Require comments and
   tracked changes to carry forward through edits, publication, restore, and
   source replacement, preserving provenance and exposing mapping conflicts.

The strongest architectural bet is a common editing engine, a CRDT focused on
live collaboration, and clear snapshot/build boundaries around it. Proposed
patches and database-owned decisions remain candidates requiring concrete
interaction and recovery designs. Language changes and server-side wasm are
implementation choices to revisit only when those boundaries demonstrate a
need.
