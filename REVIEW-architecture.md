# Simplify by giving workflows clear owners

*2026-09-14. Architectural direction after reviewing the production mutation
paths. This document describes the simplification program, not a collection of
possible future architectures.*

## Product premise

LibrePaper is primarily a collaborative editor. Publishing, review, history,
AI assistance, and local compilation grow out of that editor. Simplification
must preserve or improve the editing experience.

Comments and tracked changes must remain available against the current
document as it evolves. Preserve their identity, authorship, discussion, and
provenance. If a target is deleted or cannot be located confidently, retain
the review record and expose it as unresolved rather than dropping it or
guessing a new target.

That continuity is a product requirement which constrains architecture. It is
not itself a simplification and needs a separate correctness design.

## Diagnosis

LibrePaper does not have several competing authoritative editing engines.
The production mutation paths already converge at useful boundaries:

- Browser and filesystem peers make local CRDT transactions. The server admits
  every peer-authored source update through `Room::receive_update`.
- Native source operations use `document/session.rs`.
- HTTP and WebSocket annotation operations share `Message::into_command` and
  `Room::apply_command_with_actor`.
- Restore and source replacement share stable-identity reconciliation through
  `session::restore`.
- Agent edits, suggestion acceptance, tracked-change decisions, and
  restore ultimately mutate yrs documents through `document/session.rs` and
  the room's checked-edit path.
- Rust edit paths share the `wasm_helpers::text::Edit` representation and its
  UTF-16 coordinate convention.

The detailed inventory is in [`docs/mutation-paths.md`](docs/mutation-paths.md).

The remaining browser/server similarities are mostly compatibility and trust
boundaries, not redundant application engines. The browser uses Yjs and
CodeMirror for responsive local transactions. The server uses yrs to validate,
repair, persist, and authorize state. Both must understand the CRDT schema.
Both must perform some path checks: the browser for immediate feedback and the
server because clients are untrusted and can be obsolete.

The codebase feels unwieldy for two more immediate reasons:

1. Too many workflows are coordinated in the same UI component, especially
   `web/src/components/Reader.svelte`.
2. Several representations describe a document or build at a moment, with
   overlapping but inconsistent vocabulary and ownership.

The objective is therefore clearer state ownership and fewer durable concepts,
not one implementation of every operation.

## Architectural rules

### Classify mutations by authority

Every mutation belongs to one of three classes:

1. **Local CRDT transaction.** A browser or filesystem peer changes local
   Yjs/yrs state. It becomes authoritative only after server admission and
   durable acknowledgement.
2. **Authoritative command.** The server checks current permissions, limits,
   identity, and conflicts before committing the operation.
3. **Snapshot transformation.** An isolated function reads an immutable tree
   and produces a candidate tree, comparison, rendering, or bundle. It cannot
   commit its result to the live document.

Classification follows authority, not implementation. An authoritative
restore may use a snapshot transformation and produce a CRDT update; it is
still an authoritative command.

Do not let snapshot helpers acquire authorization or persistence. Do not make
interactive typing wait for a command API. Do not treat optimistic browser
state as authority.

### Consolidate rules within a trust boundary

Keep one authoritative implementation for:

- peer update admission and structural repair;
- revision authorship and legal state transitions;
- annotation validation and permissions;
- guarded source-edit conflicts;
- stable file identity during restore and source replacement;
- source checkpoint identity;
- publication activation and identity.

Client-side validation may repeat an authoritative rule to provide immediate
feedback. Such validation remains advisory, and the server must still enforce
the rule. Do not add a runtime boundary merely to share these checks.

### New representations must replace something

LibrePaper currently has live CRDT state, checkpoint trees, agent source trees,
browser render trees, local build results, publication display bundles, and
history comparisons. Their lifecycles are not identical, so they should not be
forced into one universal model.

Before adding a document, change, result, or bundle representation, identify:

- the existing representation it replaces; or
- the lifecycle or trust boundary that makes both representations necessary.

If a new representation cannot retire anything and does not express a new
boundary, do not add it.

### Extract ownership, not lines

Moving code into another component, module, crate, binary, or repository is
useful only when the new unit owns coherent state and has a narrow interface.
A lower line count in the original file is not sufficient evidence of
simplification.

Prefer dependency-leaf contracts. Higher-level server, browser, and CLI
orchestration may depend on them; contracts must not depend back on their
consumers.

## Workstream 1: decompose the reader by workflow

`Reader.svelte` currently coordinates collaboration, annotations, anchoring,
history, rendering, publishing, local preview, file management, AI review,
navigation, and pane layout. A change to one workflow can therefore
interact with unrelated state and lifecycle effects.

Continue extracting stateful workflow controllers under `web/src/lib/reader/`.
Each controller must:

- own one cohesive state machine or resource lifetime;
- expose a small command/query interface;
- receive external effects through explicit callbacks;
- be testable without mounting the complete reader;
- remove the corresponding mutable state and lifecycle logic from
  `Reader.svelte`.

The frame preview, frame overlays, render scheduling, render status,
diagnostics, annotation submission, collaboration, history, and local preview
controllers establish the pattern.

Prioritized remaining seams are:

1. **Publication coordinator.** Own publication metadata, source readiness,
   expected-publication identity, build/upload/activate, and refresh events.
2. **Anchor coordinator.** Own rendered and source anchoring, backfill,
   unresolved state, and re-anchoring after frame/source changes.
3. **Local build selection.** Own browser-versus-companion preferences,
   pairing state, Quarto/Calepin preview lifetimes, and capability errors.
4. **Workspace layout.** Own pane sizes, mobile view, active panel, persisted
   preferences, and layout transitions.
5. **Project files.** Own file selection, move/delete/add workflows, asset
   upload, and project download orchestration around the collaboration session.

Do not extract thin wrappers that merely forward many component variables.
When a proposed controller requires most of `Reader.svelte` as injected
getters and setters, its boundary is wrong or the prerequisite state owner has
not yet been extracted.

### Measure progress

For every extraction, record:

- mutable state and effects removed from `Reader.svelte`;
- the controller's public operations;
- tests moved or added around the workflow;
- imports and unrelated workflows no longer touched by changes in that area.

The meaningful measure is fewer reasons for `Reader.svelte` to change, not its
line count alone.

## Workstream 2: converge snapshot and result vocabulary

Several existing contracts overlap:

- the browser render tree;
- `history::Tree` and immutable source versions;
- the agent `SourceTree`;
- `results::BundleManifest` for local Quarto results;
- local companion builder responses;
- the publication manifest and display bundle.

Inventory these shapes field by field before creating another package. For
each field, record its meaning, authority, lifetime, optionality, and whether
its digest covers source, configuration, computation, or display bytes.

Converge names and small value objects where semantics match, especially:

- source revision and tree digest;
- main source path;
- artifact kind, MIME type, digest, and size;
- asset path, role, digest, and size;
- diagnostics and optional source locations;
- builder identity and provenance;
- render configuration identity.

Keep distinct top-level contracts when they have different lifecycles:

- a checkpoint is durable source history;
- a render result is transient computation output;
- a publication is an immutable display package with an authoritative active
  pointer.

The desired result is fewer conversions and less ambiguous terminology, not a
single structure with fields that most consumers ignore.

## Workstream 3: narrow the local companion boundary

The local companion should present a deliberate worker contract: a build
request and source/workspace capability go in; artifacts, diagnostics, and
provenance come out. Pairing, folder consent, cancellation, incremental
preview, and tool discovery remain explicit capabilities.

Before moving `local` into another crate:

1. Measure which ordinary changes rebuild or relink local code.
2. Identify dependencies from `local` into `results`, `quarto`, `auth`, `cli`,
   `document`, configuration, and utilities.
3. Move genuinely shared protocol and result types into dependency-leaf
   modules.
4. Make CLI/server integration depend on the worker interface rather than its
   implementation details.
5. Extract a crate only when the dependency direction is already clean and
   measurement predicts a better build or test loop.

A crate split confirms a boundary; it must not be used to simulate one.

## Workstream 4: specify review continuity separately

Review continuity is substantial product work. Its specification must cover:

- stable review-record identity and provenance;
- live CRDT-relative anchors versus publication/source selectors;
- file rename, deletion, restore, and whole-source replacement;
- tracked-change state when its target changes;
- explicit unresolved and conflict states;
- authoritative checks before any review action edits current source;
- behavior visible to collaborators and external publication reviewers.

The architecture workstreams above must not discard review records or make
their provenance unavailable. They do not need to solve every mapping case as
a prerequisite for decomposing UI workflows or converging result vocabulary.

## Testing boundaries

Use tests appropriate to each boundary:

- Browser controller tests assert workflow behavior without the full reader.
- Yjs/yrs interoperability tests assert the shared encoded document contract.
- Server tests assert hostile clients cannot bypass path, revision, quota, or
  permission checks.
- Mutation-path integration tests assert persistence precedes acknowledgement
  and authoritative compound operations commit or roll back coherently.
- Snapshot fixture tests assert equivalent projections without requiring one
  runtime implementation.
- Publication and build fixtures assert digest and provenance semantics.

Compatibility tests are not evidence of a failed architecture. They are the
appropriate mechanism when two runtimes must participate in one protocol.

## Order of work

1. Continue decomposing `Reader.svelte`, beginning with publication ownership.
2. Apply the scoped terminology and adapter decisions in the completed
   snapshot/result contract inventory.
3. Clean the local companion's dependency direction and measure whether crate
   extraction pays for itself.
4. Write and implement the separate review-continuity specification in
   product-sized increments.

After each step, prefer deleting old state, conversion code, or representations
before adding another abstraction.

## Expected outcome

LibrePaper remains a modular monolith with a collaboration-first editor. The
browser retains native Yjs and CodeMirror integration. The Rust server retains
native yrs validation and authoritative document responsibilities. Snapshots
remain explicit boundaries for history, rendering, automation candidates, and
publication.

The application becomes simpler because each workflow has one state owner,
authoritative rules have one owner within their trust boundary, and every
durable representation has a distinct purpose. Larger runtime or deployment
changes require new operational evidence; they are not part of this
simplification plan.
