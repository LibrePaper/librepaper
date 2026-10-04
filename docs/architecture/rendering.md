---
title: "Rendering and live sync"
---

## Publishing and rendering

Markdown, Typst and LaTeX are compiled by WebAssembly modules in the browser (plain WebAssembly with small exports).

LaTeX engines and TeX Live packages are fetched from an HTTPS mirror from `latex/<sha256>/` (pinned in `assets.lock`). The mirror is append-only; files are content-addressed and cached in browser storage. An operator can set `[assets].mirror` in the server TOML to host a copy.

Compilation runs in one worker per module. Markdown and Quarto produce flow HTML; LaTeX and Typst produce paged PDF. The deployment never renders or stores a compiler. The editor keeps the last successfully rendered page while the engine warms.

There is no publish step and no stored rendered page. A reader sees the same head an editor sees, projected (text tree, main file, format, assets), identified by tree digest not version number. The parent reader requests this projection from the application origin and renders it; it sends the resulting preview to the isolated document frame. Opening, reconnecting and every edit resolve to fetch-and-render.

The projection algorithm is implemented in Rust and JavaScript, held to the same behaviour by `web/tests/fixtures/projection.json`. When a document changes, the socket carries `source-changed {digest}` (digest only, not text). The reader refetches using digest as etag; unchanged files reuse cache.

Readers and commenters never receive CRDT bytes or edit history; projections carry text and assets only.

The application origin authorizes and returns projected source and assets to the parent reader app. That app renders the preview and sends rendered HTML or PDF bytes to the isolated document frame through origin-checked messaging; the frame does not receive the source projection or document access credentials in its URL. Reader link or account authority is rechecked on the application response. Assets use stable document-scoped paths with private revalidation. The projection remains readable source in the authorized reader's browser, so a reader must not treat source-only files in a shared project as private.

## Live collaboration

A room holds one document's comments and open sockets, in one process. Writes reach all readers without polling.

The socket carries document updates, presence, comments and `source-changed {digest}` notices. Presence (who is here, where is their cursor) is ephemeral and never persisted. Editors synchronise source through the socket; readers use it for annotations and digest notices. Both follow the same authority rules since readers see the same head as editors.

A dropped socket loses nothing; comments post via HTTP and reconnect hydrates a bounded first page, with older comments loaded through cursor pagination. Refreshes remain bounded. Silent disconnections (NAT expiry, sleep) are detected by periodic polling of connection liveness. Queue depth and transport writes are bounded server-side. Authority is rechecked while a socket is open.

Identity comes from GitHub or Google (browser), or the deployment's device flow (`librepaper login` in a headless terminal). Browser-started local agents receive an automatically renewed five-minute token scoped to the current document and link while the page is open. Both sign-in paths end as a handle (policy-gated) and an id (everything else keys on). Cookies use `__Host-` naming on HTTPS; OAuth uses PKCE and state cookies; logout is POST-only.

Access comes from a grant on an account or a share link. Links name a role (reader, commenter, editor), require sign-in, expire after 7 days by default, and are never logged (only hashes recorded).

An editor can make a project available offline while connected, storing application resources, editor modules, project identity and metadata locally. Offline: source edits, file creation/renames, main file choice, cached asset reads, preview work; asset uploads, decisions, restores, publishes and sharing changes require a connection. Comments are local drafts, submitted after reconnecting. Browser storage is not a backup; eviction or clearing site data can remove unsynchronised work.
