# LibrePaper simplification

*2026-09-14. Proposed implementation specification; not a claim about completed changes.*

## Purpose

Reduce the number of overlapping workflows and maintenance paths without
removing the features that make LibrePaper useful: browser collaboration,
companion rendering and chat, autonomous agent edits through MCP, offline
continuation, and future VS Code collaboration.

The immediate work is to remove unwanted terminal workflows and clarify the
responsibilities of the remaining components. It is not a server rewrite or a
replacement of the collaboration engine. A smaller help screen alone is not
success: obsolete implementations, dependencies, tests, and documentation must
also be removed where no retained feature needs them.

## Retained product

- Browser creation, upload, editing, review, sharing, and rendering.
- Server operation, configuration, status, seeding, backup, and restore.
- Companion lifecycle, local rendering tools, and supporting configuration.
- Companion chat and MCP integration, including autonomous agent edits without
  introducing a new per-edit approval step. Existing authorization remains.
- Terminal login/logout and document listing.
- One-shot export of comments or the complete project as ordinary local files.
- Browser offline continuation and the future VS Code Yjs integration.

Users do not need terminal publishing or file synchronization to use the app.
Exported files are independent copies. Changing them does not change the hosted
document.

## CLI scope

Retain the existing `admin`, `local`, `login`, `logout`, and `list` command
families where they serve the workflows above. Preserve necessary deployment
and credential selection. Do not rename working administrative and companion
commands merely to make this cleanup look larger.

Remove:

- `publish`, including both creation and replacement of existing documents.
- `sync`, including its filesystem watcher, reconciliation, baseline sidecars,
  conflict-copy machinery, and reconnect behavior where exclusively used by it.
- The top-level document `open` command. Companion URI handling is separate and
  must remain if used to launch or operate the companion.
- Human-facing terminal agent commands that duplicate companion chat or expose
  document operations as a second terminal authoring interface.
- Standalone Quarto publishing/import commands that belong to the removed
  terminal workflow. Preserve browser/companion Quarto rendering and any shared
  implementation it requires.

Inspect the `skills` command and related assets for actual runner or MCP
dependencies. Remove terminal-only discovery/export affordances; retain skill
resources and launch hooks needed by the supported agent workflow.

### MCP and process entry points

MCP is retained functionality, not an obsolete terminal feature. A process
started with command-line arguments may still be essential infrastructure.

The current `cli/mcp.rs` adapter uses `AutomationPeer` from `cli/peer.rs`, and
the `agent` command family also includes runner integration. Do not delete
these modules wholesale or infer that removing terminal sync eliminates all
non-server CRDT use.

Trace companion launchers, MCP configuration, runner lifecycle, credentials,
operation receipts, cancellation, and recovery before removing commands.
Retain required machine entry points, including their existing invocation
syntax where configured clients rely on it. Document them separately from the
ordinary user CLI. Move shared implementation out of `cli/` when that makes its
ownership clearer; do not create a new abstraction layer solely for the move.

## Export contract

Use the existing `export` subcommand rather than adding a parallel top-level
`--export` mechanism. Proposed syntax:

```sh
librepaper export DOCUMENT --comments --format markdown --output comments.md
librepaper export DOCUMENT --project --output ./paper-copy
```

`--comments` and `--project` are mutually exclusive. Omitting both retains the
current comments-export behavior. Existing comment formats (`jsonld`,
`markdown`, and `response`), checkpoint filtering, and stdout output remain
supported. Project export requires a destination directory and rejects
comment-only options such as `--since` and `--format` when explicitly supplied.

Project export downloads every source file and project-owned binary asset with
their relative paths. It does not download caches, installed toolchains,
external package repositories, or unshared files from a companion workspace.
It does not promise that a project depending on those resources will compile
on another machine. Comments remain a separate export mode.

The server captures one consistent project snapshot for the request and returns
its manifest and assets by immutable identity. Export must not fetch each source
file from independently changing live state. It includes all server-acknowledged
edits visible at snapshot capture; edits still offline are necessarily absent.
Inspect and reuse the existing snapshot/source interfaces before adding routes.

Write into a staging directory and publish the destination only after all
required files have been downloaded and verified. Refuse an existing destination
in the first implementation rather than merging or overwriting it. Validate
relative paths and prevent traversal or symlink escape. On failure, do not report
success or leave a destination that appears complete.

There is no watcher, upload, attachment, baseline, merge engine, or hidden
synchronization state. Repeating an export creates another independent copy.
Deployment backup remains separate: it preserves operational recovery data,
whereas project export produces usable project files.

## Architecture boundaries

### Keep the document-aware Rust server

Retain `yrs`, document admission, durable acknowledgement, source materialization,
history, and authoritative review operations. The server and CLI currently ship
in one binary; deleting CLI sync does not remove `yrs` from that binary.

Rendering already happens in the browser or companion. Server rendering is not
a justification for this decision. Keeping document interpretation supports
current project exports, validated changes, and server-side document operations
without requiring an editor tab to be present.

An opaque storage-and-relay server is outside this change. It would require
separate decisions about export freshness, snapshot trust, update compaction,
asset retention, review enforcement, and autonomous document operations.

### Share editing semantics

Browser editing, offline continuation, and future VS Code editing use the same
document schema and collaboration protocol. Agent operations continue through
the authorized document service and produce changes compatible with those
clients; MCP need not become a raw Yjs transport.

Share JavaScript session code where runtime semantics match. Keep browser,
extension, persistence, and transport adapters where their environments differ.
Do not add an independent file-merging engine for each client or require a
cross-language abstraction to unify Rust and JavaScript implementations.

### Preserve the companion's two roles

The companion supports both local rendering and agent/chat integration. It is
not reduced to a rendering-only service, nor required to own all collaboration.

Rendering should have a clear project-input/result contract. Audit bindings,
presets, discovery, and workspace management for duplication, but retain those
needed for local execution and projects that depend on local resources.
Simplifying these mechanisms must not silently remove supported Quarto or
other local-tool workflows.

### Separate save, history, render, and export

- Locally saved: the client's edits have completed local persistence.
- Saved to server: the server has durably acknowledged those edits.
- History version: a recoverable version recorded under the history policy.
- Render: output generated from a particular project state.
- Export: an independent copy of a captured project state.

A successful render is not evidence of persistence. Edits must not wait for a
render or downloadable archive to count as saved. This distinction does not
require removing automatic history checkpoints or rewriting the storage layer.

Audit duplicate project representations and conversions against these
responsibilities before proposing storage changes. Remove a representation only
after identifying its readers, writers, durability role, and replacement.

## Offline and VS Code requirements

Preserve [SPEC-offline-continuation.md](SPEC-offline-continuation.md) and
[SPEC-vscode-extenions.md](SPEC-vscode-extenions.md) as the detailed feature
contracts. This spec does not require implementing the extension during cleanup.

Routine disconnected edits use durable local Yjs state and reconnect through
the normal admission path. Identity changes, revoked access, or rejected updates
must preserve recoverable local work. Removing CLI sync does not remove these
recovery obligations or the extension's handling of external file modifications.

When reconciling those specs during implementation, remove assumptions that a
CLI sync process remains a supported competing writer. Clarify that describing
the companion as a build worker does not exclude its retained chat/agent role.
Do not rewrite unrelated ongoing feature work as part of this cleanup.

## Implementation sequence

1. Inventory CLI entry points and their callers, including companion launchers,
   MCP clients, installer integration, tests, documentation, and shared helpers.
   Classify each as retained user interface, required machine interface, shared
   implementation, or obsolete implementation.
2. Implement and verify complete-project export against a consistent snapshot.
   Preserve existing comments export and authentication.
3. Remove obsolete command surfaces and exclusively owned code. Preserve MCP and
   companion dependencies. Remove Cargo dependencies only after checking all
   remaining production and test uses.
4. Update help, README, manual, examples, installation guidance, and command tests
   to present server operations, companion, authentication, listing, and export.
   Removed workflows must disappear from advertised capabilities.
5. Audit save/history/export and companion configuration for further reductions.
   Record concrete redundant paths and the behavior affected before removing
   them. Do not bundle speculative storage or server rewrites into CLI removal.

## Acceptance criteria

- Login/logout, listing, all retained operator workflows, and companion startup
  work after CLI removal.
- Companion chat can launch its configured agent, connect through MCP, read the
  project, and apply authorized edits without a newly introduced approval step.
  Reconnect/retry does not duplicate acknowledged agent operations.
- Comments export preserves existing supported formats and filtering.
- Project export reproduces a consistent multi-file project with binary assets,
  including when edits occur during download. It works without an open browser.
- Export failures, invalid paths, missing assets, and an existing destination do
  not cause silent partial success or overwrite unrelated files.
- Export never sends document edits or creates a persistent file watcher.
- Removed commands are unavailable and no longer advertised. Required MCP and
  companion machine entry points remain operational.
- Browser collaboration and existing history/review tests pass. Implemented
  offline persistence/reconnect behavior remains intact, including preservation
  of rejected local work.
- Unused code and dependencies are deleted; retained shared code has identifiable
  callers. Verification targets the affected workflows rather than merely the
  new shape of the command parser.

## Non-goals

No language migration, JavaScript sidecar, removal of server-side `yrs`, opaque
server redesign, removal of companion chat or MCP, mandatory per-edit AI
approval, removal of offline continuation, or cancellation of VS Code plans.
No replacement terminal authoring workflow disguised as export. No claim that
all complexity can disappear while retaining collaboration and durable recovery.
