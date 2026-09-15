# Browser offline continuation

*2026-09-14. Proposed product and implementation specification; not a claim
about current behavior.*

## Purpose

LibrePaper is a collaborative writing and review application. A temporary loss
of connectivity must not interrupt writing or make local work disappear when
the application restarts.

The initial promise is deliberately bounded: an editor can prepare an existing
project for offline use, reopen it without the server, edit its source, preview
it with available dependencies, restart, and synchronize when connectivity
returns. Independent offline project creation is outside the initial scope.

This specification complements [REVIEW-architecture.md](REVIEW-architecture.md).
It preserves the distinction between local CRDT edits, authoritative commands,
and snapshot transformations. The related editor integration is specified in
[SPEC-vscode-extenions.md](SPEC-vscode-extenions.md).

## Product contract

1. Local typing does not wait for network access or server acknowledgement.
2. Local persistence and remote durability are distinct, observable states.
3. Previously prepared projects can be opened from an offline start page,
   including after closing every tab and restarting the browser.
4. Reconnection checks document identity and current authority before sending
   local changes. Rejection preserves recoverable local work.
5. Routine concurrent text edits converge automatically. Convergence is not a
   guarantee that concurrent prose edits preserve both writers' intentions.
6. Review records retain identity and provenance. An uncertain target becomes
   unresolved; it is never silently reassigned or discarded.

Browser storage is not a user-controlled backup. Browser eviction, clearing
site data, a lost profile, or a lost device can remove unsynchronized work.
The application must explain this when enabling offline availability and offer
ordinary source-and-asset export. A custom archive format is not required.

## Current foundations and gaps

`web/src/lib/collab.js` already uses Yjs, IndexedDB persistence, and server
acknowledgements. `web/src/lib/reader/collaboration.js` owns the room lifetime
and checks metadata on reconnection. `collab-cache.js` includes document
creation identity in cache names. Assets are referenced by digest, with their
bytes held outside the CRDT.

These mechanisms do not alone establish offline startup or complete local
availability. In particular, loading IndexedDB once does not prove that every
subsequent edit has completed a storage transaction. Implementation must audit
the actual persistence and acknowledgement boundaries before presenting save
status.

## User workflow

### Prepare

An editor selects **Make available offline** while connected. LibrePaper stores
the application resources, editor modules, project identity and metadata,
source CRDT state, and project asset bytes required for the supported workflow.
It reports progress and any missing resources. Availability is declared only
after the required writes complete.

Preview availability is reported separately by format. Compiler resources,
fonts, packages, and externally referenced resources must either be available
locally or named as missing. Source editing can be ready while preview is not.
No promise is made to download an unbounded dependency ecosystem.

Request persistent browser storage where supported and report the outcome
without treating it as protection against explicit site-data deletion.

### Continue and reopen

The cached application shell provides a list of prepared projects. Opening one
hydrates its local session without first requiring a successful metadata or
authentication request. Last-known permissions permit local continuation only;
they do not authorize writes to the server.

The editor restores source, project paths, main-file selection, and locally
available assets. Missing preview dependencies produce a specific explanation
while editing remains available. A retained preview identifies the source
version it represents and is visibly stale after edits.

### Reconnect

Reconnection revalidates the remote document incarnation and current editing
authority, then synchronizes CRDT state through the existing admission path.
It does not replace local source with a fetched snapshot.

If access has changed, authentication has expired, the document has been
deleted or recreated, or an update is refused, retain local state and explain
the next action. Authentication may be retried. A different document incarnation
must never receive the old document's cached updates. Recovery/export must be
available without successful remote authorization; do not fetch additional
remote content with stale credentials.

## Operations while offline

| Operation | Initial behavior |
| --- | --- |
| Edit existing source | Local CRDT transaction, persisted locally |
| Create, rename, move, or delete source files | Existing local CRDT operations; preserve stable file identity |
| Choose main file | Local CRDT transaction |
| Read cached assets | Available if their bytes were downloaded |
| Add or replace asset bytes | Requires connection in the first release; explain before modifying project state |
| Preview | Local computation when dependencies are available |
| Read previously cached review records | Explicitly last-known state |
| Write a comment or reply | Preserve a local draft; explicit submission after reconnect |
| Accept/reject tracked changes, restore, publish, change sharing | Requires current server authorization; no automatic delayed execution |

Existing unconfirmed submissions must remain recoverable and reconcile with
server receipts; they must not become duplicate new submissions. Offline drafts
are not authoritative review records. A general command outbox is not part of
this specification.

Existing tracked-change recording remains subject to its encoded schema and
server authorship validation. If its offline correctness cannot be established,
gate that mode explicitly rather than silently recording plain edits as tracked
changes or dropping revision metadata.

## Ownership and storage

Evolve the current collaboration controller into the owner of one local project
session. It coordinates hydration, persistence, connection, reconciliation,
and recovery independently of `Reader.svelte`. The reader renders state and
invokes operations; it does not own a second copy of mutable source.

Keep browser Yjs and server yrs. Reuse existing tree, asset, diagnostic, and
render-result contracts. Add only the offline availability manifest and storage
metadata required by this new lifecycle; do not invent another source model.

Storage must distinguish:

- Server identity and document incarnation, with account/access-context
  isolation so switching accounts does not silently expose another cache.
- Persisted CRDT state and a recoverable record of local work not yet confirmed
  by the server. Persist CRDT identity; do not regenerate edits from plain text
  after every restart.
- Asset bytes by digest, associated with the projects authorized to use them.
- Last-known project metadata and explicitly stale review data.
- Application/schema compatibility version and preview dependency availability.

Use atomic transactions or recoverable staging for related metadata changes.
An interrupted availability download must not leave a project marked complete.
Application upgrades and storage migrations must preserve unsynced work or
offer recovery before replacing incompatible state. Service-worker updates must
not forcibly reload an active editing session.

Multiple tabs must not independently corrupt storage, misattribute
acknowledgements, or report each other's pending work as remotely durable.
Logout and **Remove offline copy** need explicit handling when local work is
unconfirmed. Normal authentication expiry does not silently purge that work.

## Save and failure states

Track local persistence and remote synchronization as separate dimensions.
The UI derives concise messages from them:

| Condition | Message/behavior |
| --- | --- |
| Latest edit awaiting local persistence | Saving on this device |
| Latest edit persisted, remote confirmation outstanding | Saved on this device; offline or syncing |
| Latest local work durably acknowledged remotely | Synced |
| Local write failed or quota exhausted | Could not save on this device; offer export and retry |
| Remote update refused | Local changes not synced; retain work and explain recovery |

A connected socket, successful send, empty in-memory queue after restart, or
initial IndexedDB hydration is insufficient evidence of durability.
Define the persisted acknowledgement bookkeeping during implementation against
the existing room protocol; extend the protocol only if its receipts cannot
support the required claim. Conservative status is preferable to false success.

Before reconciliation that may remove locally edited content, preserve a
recoverable local version. CRDT state alone must not be assumed to provide a
usable recovery interface. Concurrent delete/edit and path collisions require
specific visible outcomes. Routine merges should not prompt the user.

## Delivery sequence

1. Audit and implement local-write completion, remote-confirmation bookkeeping,
   and recovery/export. Verify reload and crash behavior.
2. Add offline application startup, prepared-project discovery, and metadata
   hydration without an initial network dependency.
3. Add explicit asset/dependency preparation and honest preview availability.
4. Exercise concurrent reconnection and refusal recovery; refine explanations
   with writing trials before advertising dependable offline continuation.

Controller extraction follows these ownership needs. A companion-backed local
copy may later improve durability, but is not a prerequisite or a replacement
for validating browser behavior.

## Acceptance and release evidence

Automated tests must exercise actual browser persistence and offline startup,
in addition to controller and Yjs/yrs interoperability tests:

- Prepare a project, close all tabs, disable the network, restart the browser,
  open from the offline start page, edit, and restart again.
- Reconnect after another peer edits the same text; verify convergence,
  preservation of local work, and durability before the synced indicator.
- Drop the connection before and after server persistence but before receipt;
  restart and reconcile without losing or duplicating edits.
- Exercise concurrent rename, delete/edit, path collision, and source restore;
  verify recoverable local content and review provenance.
- Refuse access, expire authentication, recreate a slug, and reject an update;
  verify no cross-document replay or silent loss.
- Exhaust local quota, interrupt preparation, upgrade the application, and open
  multiple tabs; verify truthful status and recoverability.
- Preview with cached assets and with missing dependencies; never present old
  output as a render of the current source.

Release evidence includes observed recovery behavior in the supported browser
matrix and writing trials that measure how often users intervene and whether
they understand the explanation. Convergence tests alone do not establish a
good writing experience. Explicit clearing of site data is documented data
loss, not a scenario the application claims to survive.
