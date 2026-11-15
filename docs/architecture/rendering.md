---
title: "Rendering and live sync"
---

## Publishing and rendering

Markdown, Quarto, Org, Typst and LaTeX are compiled in the browser by plain WebAssembly modules with small exports. Each module runs in its own worker. The modules are pinned in `assets.lock`, fetched by `tools/assets/pins.mjs`, and served from the asset mirror:

- `markdown.wasm` renders Markdown and Quarto, and `org.wasm` renders Org.
- `typst.wasm` renders Typst, and `typst-syntax.wasm` gives the editor Typst's syntax tree.
- `bibliography.wasm` and `citations.wasm` format bibliographies and citations.
- `pandoc.wasm` and `harper.wasm` are used for exports and grammar checks.

Markdown, Quarto, Org and HTML produce flow HTML. LaTeX and Typst produce paged PDF. The deployment never renders or stores a compiler. The editor keeps the last successfully rendered page while an engine warms.

Quarto previews, like Markdown, Org and authored HTML previews, use the house style in `web/src/styles/document.css`. LaTeXML and Typst pages replace their own stylesheets with it.

LaTeX engines and TeX Live packages are fetched from `latex/<sha256>/` on the mirror, pinned in `assets.lock`. The mirror is append-only, and files are content-addressed and kept in Cache Storage. An operator can set `[assets].mirror` in the server TOML to host a copy.

LaTeX waits for a pause in typing before it compiles. Turn on **Update the preview while typing** in Settings, under LaTeX, to compile continuously. Changes show sooner, at the cost of one busy processor core while you type.

## Downloads

**File > Download** offers HTML, PDF and DOCX for every format, plus the source file and [comments](review.html#comments). Each file is made in the browser from the live source and is never stored. Org has a PDF too, made by Pandoc and LaTeX, or by Pandoc and Typst where no LaTeX compiler is available.

- PDF: LaTeX and Typst use their own engines. Markdown, Quarto and HTML go through Pandoc to a Typst source, which Typst compiles. Citations are formatted with the style named by `csl` or `bibliography-style`.
- HTML: each format's own HTML renderer. Authored HTML is downloaded as written.
- DOCX: Pandoc reads the HTML rendering.

When the desktop app is in use, it builds the files its commands make on this computer, instead of the browser: LaTeX PDF, Typst PDF and HTML, and Markdown or Quarto HTML, PDF and DOCX. Quarto builds locally whenever a Run directory is set. Markdown follows the Run code switch. Otherwise the browser builds the file. See [local service](local-service.md).

Pandoc is fetched from the mirror on the first PDF or DOCX that needs it, and runs in its own worker. Harper's grammar checker is fetched only when someone chooses Harper in Settings, and also runs in its own worker. Both are released by LibrePaper/wasm-upstream and pinned in `assets.lock`.

## Org

Org files (`.org`) are rendered by `org.wasm`, released by [LibrePaper/wasm-orgmode](https://github.com/LibrePaper/wasm-orgmode). Its parser began as [orgize](https://github.com/PoiScript/orgize) by Alex Lin and is developed in that repository.

The preview shows headlines with their TODO keyword and priority, `SCHEDULED`, `DEADLINE` and `CLOSED` planning lines, clock entries from a `LOGBOOK` drawer, tables with captions, footnotes collected at the end, and math typeset with KaTeX. `#+INCLUDE:` is spliced into the text before rendering, so included sections appear in the preview. `#+INCLUDE:` and `#+SETUPFILE:` also decide which file a project opens on: a file that another file includes is not a starting point.

Not supported:

- Code blocks are shown as source. Nothing is executed.
- `[cite:@key]` citations are recognised in the editor, with completion and insertion, but the preview does not format them.
- `#+OPTIONS` and macros are not interpreted.

## The live head

There is no publish step and no stored rendered page. A reader sees the same head an editor sees, projected as text tree, format and assets, and identified by tree digest rather than version number. The preview renders the file open in the editor, or the file that includes it. Opening, reconnecting and every edit fetch and render again.

The projection is implemented in Rust and JavaScript and held to the same behaviour by `web/tests/fixtures/projection.json`. When a document changes, the socket sends `source-changed {digest}` with only the digest. The reader refetches using the digest as its etag, and unchanged files come from cache.

Readers and commenters never receive CRDT bytes or edit history. Projections carry text and assets only.

The application origin authorizes the projection and returns source and assets to the parent reader app. That app renders the preview and passes the resulting HTML or PDF bytes to the isolated document frame by origin-checked messaging. The frame gets neither the source projection nor document credentials in its URL. Reader or account authority is rechecked on each application response. Assets use stable, document-scoped paths with private revalidation.

The projection stays readable source in the authorized reader's browser. Do not treat source-only files in a shared project as private.

## Live collaboration

A room holds one document's comments and open sockets in one process. Writes reach all readers without polling.

The socket carries document updates, presence, comments and `source-changed {digest}` notices. Presence says who is here and where their cursor is. It is ephemeral and never persisted. Editors synchronise source over the socket. Readers use it for annotations and digest notices. Both follow the same authority rules, since readers see the same head as editors.

A dropped socket loses nothing. Comments are posted over HTTP. On reconnect, the first page of comments loads with a bound, and older comments load by cursor pagination. Silent disconnections, such as NAT expiry or sleep, are found by polling connection liveness. Queue depth and transport writes are bounded on the server. Authority is rechecked while a socket is open.

Identity comes from GitHub or Google in the browser. A headless terminal signs in through the deployment's device flow with `librepaper login`. Browser-started local agents receive a five-minute token, renewed automatically while the page is open and scoped to the current document and link. Both sign-in paths end in a policy-gated handle that everything else keys on. Cookies use `__Host-` names on HTTPS. OAuth uses PKCE and state cookies. Logout is POST-only.

Access comes from a grant on an account or from a share link. Links name a role (reader, commenter, suggester or editor) and expire after 7 days by default. Only hashes of link keys are recorded. A link requires sign-in unless the owner chose "Anyone with the link" and the deployment's `[access]` switch for that role is on. Someone who has not signed in can at most comment, whatever the link's role.

Each project an editor opens online is kept in this browser, with application resources, editor modules, project identity and metadata, so it can be opened again without a connection. Opening a project as a reader or commenter stops this browser offering it offline. Signing out clears every local project.

Offline, source edits, file creation and renames, cached asset reads and preview all work. Asset uploads, decisions, restores, publishes and sharing changes need a connection. Comments are kept as local drafts and submitted after reconnecting. Browser storage is not a backup, and eviction or clearing site data can remove unsynchronised work.
