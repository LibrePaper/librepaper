---
title: "Rendering and live sync"
---

## Publishing and rendering

Markdown, Typst and LaTeX are compiled in the browser by plain WebAssembly modules with small exports.

LaTeX engines and TeX Live packages are fetched from an HTTPS mirror from `latex/<sha256>/`, pinned in `assets.lock`. The mirror is append-only. Files are content-addressed and cached in browser storage. An operator can set `[assets].mirror` in the server TOML to host a copy.

Compilation runs in one worker per module. Markdown and Quarto produce flow HTML. LaTeX and Typst produce paged PDF. The deployment never renders or stores a compiler. The editor keeps the last successfully rendered page while the engine warms.

## Downloads

File > Download offers Source, PDF, HTML and DOCX for every format. Each file is made in the browser from the live source and never stored.

- PDF: LaTeX and Typst use their own engines. Markdown, Quarto and HTML go through Pandoc to a Typst source, which the Typst engine compiles. Typst formats the citations, with the style named by `csl` or `bibliography-style`.
- HTML: each format's own HTML renderer. Authored HTML is downloaded as written.
- DOCX: Pandoc reads the HTML rendering.

Pandoc is the official `pandoc.wasm` build, pinned in `assets.lock` and fetched from the mirror on the first PDF or DOCX that needs it. It runs in its own worker. View > PDF for Markdown and Quarto takes the same route, so a PDF needs the desktop app only when Run code is on. Harper's grammar checker is the unmodified slim wasm from the harper.js npm package, pinned in `assets.lock` and fetched from the mirror only when someone picks Harper in Settings, and it runs in its own worker.

## The live head

There is no publish step or stored rendered page. A reader sees the same head an editor sees, projected as text tree, format and assets, identified by tree digest not version number. The preview renders the file open in the source, or the file that includes it. The parent reader requests this projection from the application origin and renders it. It sends the resulting preview to the isolated document frame. Opening, reconnecting and every edit resolve to fetch-and-render.

The projection algorithm is implemented in Rust and JavaScript, held to the same behaviour by `web/tests/fixtures/projection.json`. When a document changes, the socket carries `source-changed {digest}` with only the digest. The reader refetches using digest as etag. Unchanged files reuse cache.

Readers and commenters never receive CRDT bytes or edit history. Projections carry text and assets only.

The application origin authorizes and returns projected source and assets to the parent reader app. That app renders the preview and sends rendered HTML or PDF bytes to the isolated document frame through origin-checked messaging. The frame does not receive the source projection or document access credentials in its URL. Reader link or account authority is rechecked on the application response. Assets use stable document-scoped paths with private revalidation. The projection remains readable source in the authorized reader's browser. A reader must not treat source-only files in a shared project as private.

## Live collaboration

A room holds one document's comments and open sockets, in one process. Writes reach all readers without polling.

The socket carries document updates, presence, comments and `source-changed {digest}` notices. Presence is who is here and where their cursor is. It is ephemeral and never persisted. Editors synchronise source through the socket. Readers use it for annotations and digest notices. Both follow the same authority rules since readers see the same head as editors.

A dropped socket loses nothing. Comments post via HTTP. Reconnect hydrates a bounded first page, with older comments loaded through cursor pagination. Refreshes remain bounded. Silent disconnections, such as NAT expiry or sleep, are detected by periodic polling of connection liveness. Queue depth and transport writes are bounded server-side. Authority is rechecked while a socket is open.

Identity comes from GitHub or Google in the browser. A headless terminal uses the deployment's device flow via `librepaper login`. Browser-started local agents receive an automatically renewed five-minute token scoped to the current document and link while the page is open. Both sign-in paths end in a policy-gated handle and an id that everything else keys on. Cookies use `__Host-` naming on HTTPS. OAuth uses PKCE and state cookies. Logout is POST-only.

Access comes from a grant on an account or a share link. Links name a role (reader, commenter, editor) and expire after 7 days by default. Links are never logged. Only hashes are recorded. Edit links require sign-in. Comment links do unless the deployment allows anonymous comments.

Every project an editor opens online is kept in this browser, with application resources, editor modules, project identity and metadata. It can be opened again without a connection, and there is no button for this. Opening a project as a reader or commenter stops this browser offering it offline. Signing out clears every local project. Offline, source edits, file creation/renames, cached asset reads and preview work still work. Asset uploads, decisions, restores, publishes and sharing changes require a connection. Comments are local drafts, submitted after reconnecting. Browser storage is not a backup. Eviction or clearing site data can remove unsynchronised work.
