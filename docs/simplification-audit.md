# Simplification audit

The September 2026 CLI cleanup removed terminal publishing, filesystem sync,
top-level document opening, standalone Quarto inspection, and terminal skill
discovery/export. Their command handlers, watcher and reconciliation code,
tests owned by those modules, and the `notify` dependency were removed.

The embedded skill bundle remains a runner input: `cli/runner_context.rs` reads
`librepaper-write/SKILL.md` when it constructs an agent turn. The `agent`
command remains the machine interface used for companion chat, runner lifecycle,
candidate previews, and MCP stdio. `cli/peer.rs` and its CRDT replica therefore
remain live shared implementation.

Project export uses the existing authenticated snapshot route. That route
captures source text, the file manifest, comments, and the live tree identity
while holding the room and bundle locks. Text is carried in that response;
binary assets are fetched by the immutable SHA-256 identity recorded in the
captured tree. The CLI checks paths, sizes, and hashes in a sibling staging
directory before renaming it to a destination that must not exist.

The following representations still have distinct responsibilities:

- The live Loro session is the collaborative and offline-compatible editing
  state.
- A history tree identifies a recoverable source version and its immutable
  assets.
- Bundle manifests identify rendered display output.
- Companion job manifests describe isolated local build inputs and results.
- Project export is a disposable filesystem projection of one captured tree.

No further representation can be removed without changing one of those
durability, history, rendering, or local execution contracts. Companion presets,
Quarto bindings, discovery, and isolated workspaces remain because local builds
and projects with unshared resources read them directly.
