---
title: "Rendering and live sync"
---

## Publishing and rendering

Markdown, Typst and LaTeX are compiled by WebAssembly modules in the browser (plain WebAssembly with small exports). Each module's URL carries a digest; the same pinned releases back native command-line tools.

LaTeX engines and TeX Live packages are fetched from an HTTPS mirror from `latex/<sha256>/` (pinned in `assets.lock`). The mirror is append-only; files are content-addressed and cached in browser storage. An operator can host a copy with `--asset-mirror URL`.

Compilation runs in one worker per module. Markdown and Quarto produce flow HTML; LaTeX and Typst produce paged PDF. The deployment never renders or stores a compiler. The editor keeps the last successfully rendered page while the engine warms.

Quarto is handled as a browser-side subset (no code execution, filters, shortcodes or JavaScript). A `.qmd` is kept as source with a short-lived draft representation for annotation mapping.

There is no publish step and no stored rendered page. A reader sees the same head an editor sees, projected (text tree, main file, format, assets), identified by tree digest not version number. Opening, reconnecting and every edit resolve to fetch-and-render.

The projection algorithm is implemented in Rust and JavaScript, held to the same behaviour by `web/tests/fixtures/projection.json`. When a document changes, the socket carries `source-changed {digest}` (digest only, not text). The reader refetches using digest as etag; unchanged files reuse cache.

Readers and commenters never receive CRDT bytes or edit history; projections carry text and assets only.

Projected source and assets are served only on the document origin, never the application origin. Reader link or account authority is rechecked on every response. Assets use stable document-scoped paths with private revalidation.

## Live collaboration

A room holds one document's comments and open sockets, in one process. Writes reach all readers without polling.

The socket carries document updates, presence, comments and `source-changed {digest}` notices. Presence (who is here, where is their cursor) is ephemeral and never persisted. Editors synchronise source through the socket; readers use it for annotations and digest notices. Both follow the same authority rules since readers see the same head as editors.

Source is edited in CodeMirror 6, bound to the shared document. Each editor keeps their own undo history and sees others' carets where they are.

A dropped socket loses nothing; comments post via HTTP and reconnect resends the full list. Silent disconnections (NAT expiry, sleep) are detected by periodic polling of connection liveness. Queue depth and transport writes are bounded server-side. Authority is rechecked while a socket is open.

Identity comes from GitHub or Google (browser), or the deployment's device flow (`librepaper login` in a headless terminal). Browser-started local agents receive an automatically renewed five-minute token scoped to the current document and link while the page is open. Both sign-in paths end as a handle (policy-gated) and an id (everything else keys on). Cookies use `__Host-` naming on HTTPS; OAuth uses PKCE and state cookies; logout is POST-only.

Access comes from a grant on an account or a share link. Links name a role (reader, commenter, editor); a signed-in account must present the link to use it. Links expire after 7 days by default. The share dialog offers renewal (new key) and rotation (revoke and mint together). Keys never reach logs; only hashes are recorded.

An editor can make a project available offline while connected. LibrePaper stores application resources, editor modules, project identity and metadata locally, opened from an offline start page. While offline: source edits, file creation/renames, main file choice, cached asset reads, and preview with available dependencies work. Asset uploads need connection. Comments are local drafts and submitted explicitly after reconnecting. Decisions, restores, publishes and sharing changes require server authority and are not queued.

Local persistence and remote durability are separate; the interface reports both. A connected socket or empty queue after restart is not evidence of durability. On reconnection, document identity and current authority are checked before sending local changes. Browser storage is not a backup; eviction or clearing site data can remove unsynchronised work. Local caches are isolated by account and access context.
