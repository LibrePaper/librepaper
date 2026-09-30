---
title: "Architecture"
---

## Where code runs

| Where | What happens there |
| --- | --- |
| The browser | Editing, typesetting, rendering, offline copies |
| The server | Identity, authority, durability, live relay, review decisions |
| The author's machine | Quarto, Typst to HTML, Zotero |

Typesetting runs in WebAssembly modules in the browser. The deployment never compiles documents or stores compilers. LaTeX engines and TeX Live packages are fetched from an HTTPS mirror (configurable). Authority is decided server-side on every request, never inferred from the browser. The optional companion app (`librepaper local start`) handles Quarto (code execution) and Typst to self-contained HTML; it reaches Zotero read-only and never runs TeX.

## Records and caches

Records describe what somebody did at a moment and cannot be edited afterwards. Caches describe where something is now and are recomputed on every document change. A comment that loses its place reports that rather than being re-pointed.

## Trust model

1. The operator sees every draft, comment, identity and presence event in clear (no end-to-end encryption).
2. The document owner holds every authority.
3. A link holder holds exactly the role their link names; links expire.
4. Documents are hostile; bytes uploaded run their own scripts inside the reader.

Published documents are served from a second configured origin (separate host required). Documents may run their own code but not fetch code from other hosts. Messages between reader and document are origin-checked at both ends.

## The document

A directory is held as four maps in one CRDT document:

| map | keys | values |
| --- | --- | --- |
| `files` | file id | text |
| `paths` | file id | path |
| `assets` | path | digest of bytes |
| `meta` | `main`, engine, markers | document settings |

Text is stored under an opaque id; paths are separate entries pointing at ids. Renames move strings in `paths` without touching text. Comments name files by id, so renames don't orphan reviews. Figure bytes sit in the object store under their digest; identical uploads are stored once. The editable source is authoritative; comment ranges are translated to source when made.

The CRDT is a single Rust library linked natively on the server and compiled to WebAssembly in the browser. Every offset counts UTF-16 code units (matching browser counts). Cursors use Unicode code points; diffs use code points (server) or UTF-16 (browser). Diff information never crosses the network; each side computes from its own state. Both are pinned by tests.

Each document's log is capped by `log_quota_mb` (advanced config, default 32 MB). New edits are refused when the limit is reached. Deployments whose limits cannot accept work they cannot save are refused at startup.

## Storage and history

PostgreSQL holds accounts, documents, grants, share links, annotations, replies, update logs, labels, proposals, hunk decisions, and storage accounting. The object store holds immutable blobs: collaboration bases, source archives, document assets and published files, keyed by content digest or never-reused names.

Document state is persisted as a compressed base plus an ordered log in Postgres, compacted periodically. The base is full operation history compressed with zstd (a million keystrokes costs a few hundred kilobytes). There is no retention window on edit history.

A checkpoint records the whole directory as a canonical tree. Checkpoints are written when somebody labels a moment, restores an earlier version, commits from the command line, or a proposal is accepted. The manifest is served to the browser. Checkpoints can be labelled and are kept until the document is deleted.

Source archives are produced on request (keyed by tree digest), not at checkpoint time. They are content-addressed and the fastest way to retrieve a document at a point in time.

Document assets are content-addressed; identical uploads are stored once. Uploads reserve quota before the blob write begins and release it if cancelled.

Storage limits:
- `--publisher-storage-limit`
- `--deployment-storage-limit`
- `--publisher-upload-limit`
- `log_quota_mb` (advanced config)

Owners can set softer history budgets and retention thresholds but cannot raise deployment hard limits. Documents may expire based on creation or last edit.

`librepaper admin backup` writes a snapshot-consistent Postgres dump plus referenced objects and verifies it. `restore` restores into a fresh directory.

Maintenance (compaction, archiving, deletion) runs on an in-process bounded queue. An idle deployment issues no maintenance queries. Reclamation releases storage only after physical deletion. Superseded objects become eligible after a grace period.

## Comments

Three things are kept about each comment:

| | what it is | lifetime |
| --- | --- | --- |
| Original anchor | what the reviewer selected in the document as it stood | written once, never modified |
| Derived attachment | where that passage is now | cache, recomputed on every edit |
| Presentation context | rendered quotation | display evidence, never identity |

The original anchor is a checkpoint plus a UTF-16 range or the whole document. Every anchor is a range (no point annotations).

Locating: The server bridges rendered page to source by flattening both with syntax blanked and whitespace collapsed. Selection is looked for in the flattened source, mapping positions back. It refuses to locate if a passage reads identically in several places.

Resolving: Cursors follow characters through insertions and deletions. Resolution falls back to searching for quoted words if cursors are unusable. Two equally good candidates produce `ambiguous`. Replacement cursors are stored in cache, never anchor. Results are attached at position, deleted, or ambiguous.

Marks are painted in the browser; source identity and current attachment come from the server.

Comments follow the W3C Web Annotation Data Model (reshaping, not translation). `librepaper export DOCUMENT` writes them in Markdown or JSON.

## Track changes

A tracked change is a branch: a fork of the document at a known point. Everything awaiting a decision is one object:
- typing with **Track changes** on (usually one hunk)
- a suggestion from a comment thread (one hunk)
- an agent run (many hunks)

All appear in the **Changes** pane. The stored object is called a proposal. A branch is stored as a blob of operations alongside a Postgres row (not a room, no sockets, nothing inside the shared document).

A hunk is a maximal run of non-retain operations. Replacing "cat" with "tabby" is one decision, not two. Character-level raw differences are grouped: two changes separated by eight or fewer retained units are one hunk. A decision names a hunk by branch, base, tip and index.

Accepting a whole branch imports it. Accepting part of one: (1) import the branch, (2) compute inverse difference, (3) keep only rejected hunks, (4) apply as reviewer. Result is the accepted subset with accepted text attributed to author and removal to reviewer. Steps 1-4 are atomic; intermediate state is never persisted.

The server counts in code points; the browser counts UTF-16. No diff information crosses the network; each side computes from its own state.

Two branches touching the same passage are grouped into one card. Adjacent edits are not grouped. Stale rows are excluded.

Any subset of open changes can be merged into a scratch document and read as finished prose (speculative reading). Attribution survives partial accept; operation history is fully reachable.

## Publishing and rendering

Markdown, Typst and LaTeX are compiled by WebAssembly modules in the browser (plain WebAssembly with small exports). Each module's URL carries a digest; the same pinned releases back native command-line tools.

LaTeX engines and TeX Live packages are fetched from an HTTPS mirror from `latex/<sha256>/` (pinned in `assets.lock`). The mirror is append-only; files are content-addressed and cached in browser storage. An operator can host a copy with `--asset-mirror URL`.

Compilation runs in one worker per module. Markdown and Quarto produce flow HTML; LaTeX and Typst produce paged PDF. The deployment never renders or stores a compiler. The editor keeps the last successfully rendered page while the engine warms.

Quarto is handled as a browser-side subset (no code execution, filters, shortcodes or JavaScript). A `.qmd` is kept as source with a short-lived draft representation for annotation mapping.

There is no publish step and no stored rendered page. A reader sees the same head an editor sees, projected (text tree, main file, format, assets), identified by tree digest not version number. Opening, reconnecting and every edit resolve to fetch-and-render.

The projection algorithm is implemented in Rust and JavaScript, held to the same behaviour by `web/tests/fixtures/projection.json`. When a document changes, the socket carries `source-changed {digest}` (digest only, not text). The reader refetches using digest as etag; unchanged files reuse cache.

Readers and commenters never receive CRDT bytes or edit history; projections carry text and assets only.

Projected source and assets are served only on the document origin, never the application origin. Reader link or account authority is rechecked on every response. Assets use stable document-scoped paths with private revalidation. Documents may run their own code but not fetch code from other hosts.

## Live collaboration

A room holds one document's comments and open sockets, in one process. Writes reach all readers without polling.

The socket carries document updates, presence, comments and `source-changed {digest}` notices. Presence (who is here, where is their cursor) is ephemeral and never persisted. Editors synchronise source through the socket; readers use it for annotations and digest notices. Both follow the same authority rules since readers see the same head as editors.

Source is edited in CodeMirror 6, bound to the shared document. Each editor keeps their own undo history and sees others' carets where they are.

A dropped socket loses nothing; comments post via HTTP and reconnect resends the full list. Silent disconnections (NAT expiry, sleep) are detected by periodic polling of connection liveness. Queue depth and transport writes are bounded server-side. Authority is rechecked while a socket is open.

Identity comes from GitHub or Google (browser), or the deployment's device flow (terminal). Both end as a handle (policy-gated) and an id (everything else keys on). Cookies use `__Host-` naming on HTTPS; OAuth uses PKCE and state cookies; logout is POST-only.

Access comes from a grant on an account or a share link. Links name a role (reader, commenter, editor); possession is the grant. Links expire by default. The share dialog offers renewal (new key) and rotation (revoke and mint together). Keys never reach logs; only hashes are recorded.

An editor can make a project available offline while connected. LibrePaper stores application resources, editor modules, project identity and metadata locally, opened from an offline start page. While offline: source edits, file creation/renames, main file choice, cached asset reads, and preview with available dependencies work. Asset uploads need connection. Comments are local drafts and submitted explicitly after reconnecting. Decisions, restores, publishes and sharing changes require server authority and are not queued.

Local persistence and remote durability are separate; the interface reports both. A connected socket or empty queue after restart is not evidence of durability. On reconnection, document identity and current authority are checked before sending local changes. Browser storage is not a backup; eviction or clearing site data can remove unsynchronised work. Local caches are isolated by account and access context.

## Companion app

`librepaper local start` runs a loopback service executing tools on the author's machine. Discovery resolves local tools to absolute paths.

The companion handles two cases the browser cannot: Quarto (code execution) and Calepin (self-contained HTML from Typst with embedded images). It reaches Zotero read-only. Everything else compiles in the browser. Typst produces PDF there; LaTeX and Biber are WebAssembly only.

Versioned protocol over loopback HTTP with bounded limits on body, upload, file count, PDF, log size and job deadline. Health endpoint identifies the service and negotiates version (nothing else). Requests with mismatched `Host` header are answered before parsing (DNS rebinding protection).

A browser pairs with the service per origin and project. Pairings live in the user's state directory keyed by origin and project. Only the token hash is written to disk; tokens expire.

A browser cannot enumerate applications; the client makes one bounded probe and remembers the outcome.

Presets are owned by the companion. Only opaque id and metadata cross to the browser; paths, arguments and environment stay on the machine. Presets may not set loader or interpreter environment variables.

An agent edits via MCP to the server (not loopback). MCP owns transport; the room owns effects and durability. The patch language is independent of MCP. A caller supplies source tree identity and immutable byte ranges; the validator produces a new source without guessing anchors. Patches whose ranges don't match are refused. Validated batches apply across all text files in one commit. The operation persists before changes; the receipt commits only after durability.

Agent work arrives as a [tracked change](#track-changes) branch and is reviewed and decided like human suggestions. Nothing enters the document without a decision. An agent's authority is its link; the caller's session never widens it.

The private assistant channel is a short-lived rendezvous between browser and runner. The server relays bounded events while both sockets are connected. It is not a queue or transcript store.

A document is written by editor-access holders; an agent reading one reads untrusted text. Agents on shared documents should be treated as acting on everyone's instructions. The companion executes code from anyone with editor access; running it against shared documents means running collaborators' code on your machine.

## The interface

Pages use [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton provides buttons, cards, inputs, tables, dialogs, tooltips and toasts. `web/src/styles/theme.css` colours everything from four colours (palette written once). Three rules, enforced by `make test`:

1. Colours and sizes come from theme, not hex values or arbitrary Tailwind sizes.
2. A control is a component; there is one `IconButton`.
3. Layout comes from `Page`, `Stack` and `Row`, assembled not measured.

Agent highlights are drawn on the document origin (outside this stylesheet). Identifying colours in shared sessions travel over the wire (not a local decision).

## Building from source

The binary embeds the web build and pinned browser renderers.

| Built by |
| --- |
| `web/` (Svelte, Skeleton, CodeMirror 6, Loro): bun and vite |
| `crates/librepaper/` (server and CLI): cargo |

Renderer implementations live in `wasm-*` repositories. Cargo links pinned native libraries; the browser uses WebAssembly artifacts from the same release tags. The web build writes to `web/dist`, which the binary embeds.

```sh
make web                # pages from web/
tools/pins fetch        # pinned browser renderers
make build              # dist/librepaper with pages and renderers
make install            # to ~/.local/bin (override PREFIX= or BINDIR=)
make test               # rustfmt, clippy and test suite
tools/suite external    # Quarto/R/Python and local-service integrations
tools/suite workloads   # supported limits and diagnostics
```

Build needs [bun](https://bun.sh) and Node.js. Browser renderers are fetched from exact tags and SHA256 digests in `assets.lock`. `tools/pins check` verifies tags match native dependencies without network access. To update a renderer: `tools/pins update wasm wasm-markdown v0.2.0`, then review Cargo and lockfile diff.

The four browser modules (markdown, bibliography, citations, typst) are not embedded. `tools/pins fetch` fetches them to `web/wasm/` (ignored). `deploy/assets publish` publishes to the asset mirror at `wasm/<sha256>/<module>` (SHA256 from `assets.lock`). The same lock pins LaTeX at `latex/<sha256>/`. The server passes these URLs to browsers on the mirror named by `--asset-mirror`. See [asset mirrors](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md).

`make demo` runs the site, application, local companion and simulated activity. It runs `deploy/assets check` on the LaTeX mirror at `MIRROR=` (default `../wasm-latex/mirror`), then `make serve` with Docker PostgreSQL from `tools/db dev` (or `LIBREPAPER_DATABASE_URL`). Configure OAuth in `.env` (see `.env.example`; `deploy/keys shell` loads deployment keys). Every new account gets five private editable examples (HTML, Markdown, Typst, LaTeX, Quarto). Use `PUBLISHERS=any COMMENTERS=anyone` for local development. Examples are created once per account, survive restarts, and stay deleted if removed. Existing accounts are unchanged; the catalogue is never reset or seeded. Starter sources ship in the binary.
