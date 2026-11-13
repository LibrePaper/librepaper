# Render transport: getting a project to its renderers

Status: proposed 2026-11-11, revised 2026-11-13. This replaces the first draft (`e769c89ae`). Section 12 lists what changed and why.

A render reads a project: texts, figures, fonts, bibliographies, styles. Today every preview render collects the whole project, fetches every binary asset at once, and hands all of it to the renderer. On a large project that is slow, and the parallel fetches can exhaust the server's transfer permits, so the server answers 429 to page opens. This document specifies how assets are fetched and kept, how the work a render repeats is cut, and how a project's assets are loaded: whole, once, when it opens.

No client in this design caps how many requests it sends. The limit that 429s protect belongs to the server, and the client cannot see it. So the server stops relaying bytes it does not need to relay (section 6.2), and where it must relay them it makes a request wait for a permit instead of refusing it (section 6.3).

PR 94 (`renderScope`, not merged) found the problem. On a project of 704 texts and 84 assets, a page open fetched all 84 assets and sometimes drew 429s, and a file switch was slow. The PR's fix hands the renderer an allow-list of files. It drops files that compilers do read (section 3.9). This spec keeps the PR's goal and reaches it without ever withholding a file on a guess.

## 1. Product principle

> A render reads the whole project. A project is loaded once, when it opens, the same way for every format. After that a render pays only for what changed.

Four rules follow from it:

1. **The renderer's input never shrinks on a guess.** No file is withheld because a heuristic judged it unrelated. Work is saved one way only: by not repeating it.
2. **Bytes travel once where the browser can keep them.** An asset is addressed by its SHA-256. A browser that has fetched and stored it does not fetch it again until the person signs out. Three ordinary conditions fetch again and are accepted: an entry the store evicted, a browser that refuses storage, and two tabs that open cold at the same moment.
3. **Nothing private outlives sign-out.** Whatever this browser keeps of a project is deleted when the person signs out, the way projects and rendered pages already are (`web/src/lib/local-data.js`).
4. **Measure, then change.** Every phase after the first starts from a profile and changes only the steps the profile shows to matter.

## 2. Scope

### In scope

- One client asset store: fetching through signed links, retries, and a persistent store that sign-out deletes (section 5).
- Serving assets: signed links to object storage, a wait for a permit where the server relays bytes, and refusals that say how long to wait (section 6).
- A profile of one render, and staging that sends a renderer only what changed (section 7).
- Loading a project whole when it opens, and keeping it loaded while it changes, the same way for every format (section 8).

### Out of scope

- The room protocol (`librepaper.room.v3`) and how texts reach the browser. Texts are already local when a render starts.
- The asset mirror (`docs/dev/asset-mirrors.md`). It serves pinned engine modules, fonts and LaTeX release files, not project assets.
- Server-side renders, such as MCP candidate renders (`crates/librepaper-server/src/server/mcp/render.rs`).
- The desktop app's local renders. They keep the whole tree and today's upload (section 3.10).
- Everything in section 10: a scope built from project structure, loading files as a render reads them, a render identity over read files, a per-principal share of transfers, a digest plan for desktop uploads, and a batch endpoint. Each is deferred with the evidence that would reopen it.

## 3. Current behavior and constraints

### 3.1 The tree

The preview builds one tree per render and reads it once (`web/src/lib/reader/preview-render.js:148`).

- `session.tree()` (`web/src/lib/project-session.js:915-952`) returns `{ texts, digests, files, retained }` for the whole directory. It is cached on the Loro oplog version plus the projection rules, so the cache misses after every edit.
- Without a session, `projection.tree()` (`web/src/lib/reader/projection-source.svelte.js:141-155`) returns `{ main: "", texts, retained, digests, settings }`.
- `Reader.svelte` overlays the editor's live text (`liveTreeNow`) and adds `main: previewPath` (`treeNow`). `previewPath` comes from `previewTarget` in `web/src/lib/opening.js:290-312`.

`digests` maps every binary asset's path to its SHA-256. `texts` holds every text. Styles, bibliographies and packages are texts: the text extensions include `.csl`, `.lua`, `.bib`, `.sty`, `.cls` and `.bst` (`crates/librepaper-base/src/config/mod.rs:317`).

### 3.2 What each renderer receives, and what it reads

Every renderer is handed every text and every gathered asset by value (`web/src/lib/renderers.js:186-189`). What each one reads is narrower:

| Renderer | How it receives the tree | What it reads |
|---|---|---|
| Markdown, Org | `renderTree` copied to a worker. `handOver` calls `clear_files`, then `add_file` for each file, `set_asset_url` for each asset, `set_main` (`renderers.js:325-336`, `renderer-wasm.js:150-167`) | The main source and the URL map. The module compiles `(source, title, resolver)` and rewrites each relative `src` through the resolver (`wasm-markdown/src/markdown.rs:104-150`, `wasm-orgmode/src/render.rs:131`). It never reads the files handed over. The citations module reads texts (`wasm-bibliography/src/abi.rs:30`) |
| Quarto (browser draft) | `composeDraft` builds a Markdown draft in JS from the texts and the URL map, then the Markdown worker renders it (`engines/quarto.js:387, 547`) | Texts for includes, and the URL map. It "deliberately does not execute code, filters, shortcodes, or JavaScript" (`engines/quarto.js:6`), so it reads no CSS, SCSS, Lua or `_extensions` |
| Authored HTML | `resolveHtmlAssetUrls` rewrites `src`, `poster` and `data` through the URL map, and inlines nested frames (`renderers.js:90-120`) | The main source, nested HTML texts, and the URL map. It cannot fail |
| Typst (PDF or HTML), and Markdown or Quarto to PDF through Typst | The same hand-over as Markdown | Project files on demand, as bytes. A file that is not there is `FileError::NotFound` (`wasm-typst/src/typst.rs:317-322`) |
| LaTeX to PDF (wasm) | Worker `stage` message. `flushCache`, then `writeTree` writes every text and asset into each engine's file system (`latex/worker.js:308-337, 597-601`) | Files on demand from that file system |
| LaTeX to HTML (LaTeXML) | `{...tree.texts, ...tree.assets}` written the same way (`latex/html.js:254-260`) | Files on demand |
| Desktop app (protocol 3) | Multipart POST of every text and asset with a `{path, sha256, size}` manifest (`desktop/client.js:418-430, 515-547`) | Anything: it runs the person's code |

The first three rows produce **flow pages**: HTML painted in the documents frame. The frame is on another origin, which is why it is given data: URLs and never a blob: URL or the asset route (`web/src/lib/figures.js:9-17`).

### 3.3 Fetching assets

`gather` (`web/src/lib/figures.js:89-136`) runs an unbounded `Promise.all` over every entry in `digests`.

- `fetchOne` (33-58) requests `/api/documents/{slug}/assets/{sha}` with `cache: "no-store"` and verifies the SHA-256.
- The only cache is an in-memory `Map` keyed by document, credential and digest, which also merges in-flight requests. It dies with the tab.
- `gather` builds a base64 data: URL for every asset, whether or not the render will use one. LaTeX and Typst never do.
- There is no retry. A failed fetch becomes `missing`, and paged output and LaTeX HTML then throw (`preview-render.js:260-263`). A flow page renders anyway and shows a broken image.

### 3.4 Serving assets

`handle_asset_read` (`crates/librepaper-server/src/server/figures.rs:147-222`) checks read access. Readers below Editor also need `room.references_asset(sha)`. It then streams through `cost::blob_response` (`cost.rs:629`).

- **The server relays every byte.** Asset bytes live in the blob store: an S3 bucket in production (`tools/deploy/production.toml`, `object_store = "s3"`, OVH in `bhs`), a directory in the self-hosting kit's default. `blob_response` reads the object in 1 MiB ranges and streams them out. No route hands the browser a link to the bucket.
- **Uploads carry no type.** `ObjectBlobStore::put_new` ignores its `content_type` argument (`crates/librepaper-engine/src/storage/blob.rs:243`), and keys are digests with no extension.

- **Transfer permits:** `DEFAULT_ARTIFACT_TRANSFERS = 64` (`crates/librepaper-base/src/config/mod.rs:183`) is one semaphore for the whole deployment. It covers `/api/.../assets/`, and also `/published/` and `/assets/`, the web app's own bundles (`cost.rs:509-511`). A permit is held until the body finishes streaming. The number dates from the initial commit, documented as "compiler/font/artifact transfers", from before engines and fonts moved to the mirror. Nothing records how it was chosen.
- **Work permits:** `work_concurrency` is 64 as well, taken for every request and held until the handler answers. It is taken first (`cost.rs:425`), so a request that reaches the transfer check already holds one.
- **Request budget:** `requests_per_principal_minute` is 6000, keyed by account, or by network for a request without one (`cost.rs:94-99`).
- **Refusals:** a permit that is not free is refused at once, with no wait. Every refusal answers 429 with `retry-after: 60` and a `reason` of `transfer_concurrency`, `work_concurrency` or `request_budget` (`cost.rs:224-232`). For the two concurrency reasons 60 seconds is wrong: a permit frees in milliseconds.
- **Why one client hits it:** over HTTP/2 a client that opens 84 fetches at once asks for more than the deployment's 64 transfer permits.

### 3.5 Asset responses are not cached, on purpose

`blob_response` sets `cache-control: private, no-store` itself (`cost.rs:713`), and the client asks for `no-store`. The comment on `handle_asset_read` that says the bytes are "cached for a year" (`figures.rs:144`) is wrong.

The header is right and stays. A browser's HTTP cache outlives sign-out, which the server already says where it refuses to cache a keyed page (`routes.rs:867`). The `Clear-Site-Data: "cache"` header that could empty it is not something every browser honours: Firefox removed the `"cache"` directive and Safari's support is not established. A year-long `immutable` entry would also answer without the server's access check. So the HTTP cache cannot hold private asset bytes, and the store in section 5 is one the app deletes itself.

### 3.6 What this browser already keeps

- **Projects and rendered pages:** two IndexedDB databases, `librepaper-offline-v2` and `librepaper-preview-v1`. The first holds document states and updates, not asset bytes (`web/src/lib/offline-projects.js`). `forgetLocalData` deletes both at sign-out and tells other tabs over a `BroadcastChannel` (`web/src/lib/local-data.js`). Writers check `localDataForgotten()` first, so a signed-out tab cannot recreate what the sign-out deleted.
- **Stand-in pages:** `reader/preview-cache.js` keeps the last page drawn per file, at most 48 pages and 96 MiB. A stored page already contains its figures, as data: URLs.
- **Public immutable files:** `web/src/lib/cache.js` keeps Typst packages in Cache Storage under the bare URL. It is never cleared and has no size bound, which suits public files and nothing else.

### 3.7 Render identity and caches

- **Snapshot digest:** `snapshotDigest(tree)` (`web/src/lib/projection-digest.js:75-89`) hashes `main` and every file in the project, one awaited SHA-256 per text.
- **Finished renders:** a match in the in-memory `finished` map (`preview-render.js:102-113`, 8 entries) skips the compile and the fetch. Its key includes that identity (208).
- **LaTeX results:** a LaTeX result is accepted only if the identity still matches (`preview-render.js:307-309`), which hashes the tree again.

### 3.8 Per-render work that grows with the project

Engines open only what they read, so a large project does not make the compile itself slower. These steps do grow with the project, on every render:

1. `includersOf` parses every text to choose the preview target (`opening.js:290`).
2. `snapshotDigest` hashes every text, up to three times in one render.
3. `renderTree` copies every text, every asset and every data: URL to the worker (`renderers.js:325-336`).
4. `handOver` clears the module's file map and adds every file again (`renderer-wasm.js:150-167`).
5. LaTeX flushes each engine's file system and writes the whole tree to each (`latex/worker.js:308-337`).
6. `gather` encodes a data: URL for every asset the first time it is held.

Nobody has measured which of these matter. PR 94 reports "~2.7s" for a file switch after its change, with no figure from before and no breakdown.

### 3.9 Why a guessed scope is unsafe

A scope built from includes, extensions or name matching withholds files a render reads:

| Read | Format | Why a structural scope misses it |
|---|---|---|
| `\lstinputlisting`, `\inputminted`, `\import{dir}{file}`, files loaded by a local `.sty` | LaTeX | Not an include command (`includesOf` follows `\input`, `\include`, `\subfile` only) |
| `main.tex`, read by a chapter that starts `\documentclass[../main.tex]{subfiles}` | LaTeX | The chapter does not include it |
| `\InputIfFileExists{notes.md}{}{}` | LaTeX | Not an include command, and `notes.md` counts as a document of its own. Without the file the compile succeeds and the text is gone |
| `read("notes.txt")`, `image("figs/" + name + ".png")` | Typst | Computed or unlisted paths |
| `<img src="my%20plot.png">` for the file `my plot.png` | HTML, Markdown | The name in the source is not the name of the file |
| A header or template in `.html` | Any | Every HTML file counts as a document of its own (`opening.js:227-243`) |

A fall-back that reruns with the whole tree when a render fails does not cover these, because several of them do not fail. A flow page with a missing image renders without error. `\IfFileExists` takes its other branch. The result is a page that is silently different, which is worse than a slow one. Hence rule 1.

### 3.10 The desktop app

Local renders run the person's code, which can read anything, and `sync_hosted_workspace` deletes files the manifest leaves out (`crates/librepaper-companion/src/local/render/workspace.rs:282-330`). They keep the whole tree.

The upload is not a cost worth designing for. A job is capped at 64 MiB and 2000 files (`crates/librepaper-companion/src/local/protocol.rs:19-21`), it travels over loopback, and the workspace persists between jobs and leaves unchanged files untouched. What a local render does pay for is fetching the assets from the server, which section 5 fixes.

### 3.11 What engines report about their reads

- **Markdown, Org, Quarto draft, authored HTML:** the output is the report. Every asset the page shows is in it, as the URL the resolver returned.
- **Typst:** `needs` reports packages and fonts it did not find (`needs.js`). It does not report project files, found or not.
- **LaTeX:** the engine's reply carries `inputFiles` (`latex/driver.js:381`), used today only to find nested `.aux` files. No test in this repository covers it, and whether LaTeXML sets it is not known. The engine also streams resolver evidence for lookups that miss (`latex/driver.js:266-281`). Whether a miss on a project path reaches that stream is not verified.

## 4. Vocabulary

- **Asset:** a binary file in the project, addressed by SHA-256 (`tree.digests`).
- **Asset store:** the one client module that fetches, verifies and keeps asset bytes (section 5).
- **Flow page:** HTML output from the Markdown, Org, Quarto draft or authored HTML renderers, painted in the documents frame.
- **Compiled render:** any other preview render: Typst, LaTeX, Markdown or Quarto to PDF, and every render on this computer.
- **Staging:** copying the tree into a renderer: the message to the worker and the writes into the module or engine.

## 5. The client asset store

Phase 1. `web/src/lib/figures.js` becomes the asset store. Every caller that needs asset bytes goes through it: previews, candidate previews, exports, downloads, workspace previews and `figureUrlFor`.

### 5.1 Fetching

The store sends every request it needs at once. It does not cap requests in flight: the capacity that matters is the server's and the bucket's, the client cannot see either, and section 6 makes both answer a burst without refusing it. Requests for the same digest are still merged.

For each asset not in memory or in the persistent store:

1. **Ask for a link.** `GET /api/documents/{slug}/assets/{sha}/link` (6.2). The answer is `{"url": "<signed link>"}` or `{"url": null}`.
2. **Fetch the link** with a plain `fetch(url)`: no credentials and no custom headers, so the browser sends a simple CORS request and no preflight.
3. **Fall back to the byte route** when the answer is `null` (the store keeps files on disk), or when the fetch from the bucket fails as a network error (a bucket without the CORS rule of 6.2 fails this way). After either, the tab uses the byte route, today's `/api/documents/{slug}/assets/{sha}`, for the rest of its life, so a deployment without links costs one extra request per tab, not one per asset.
4. **Verify** the body against its digest, wherever it came from.

### 5.2 Retries

| Response | What the store does |
|---|---|
| 403 from the bucket | The link expired or was refused. Ask for a new link once, then fall back to the byte route |
| 429 with `reason` `transfer_concurrency` or `work_concurrency` | Wait for `Retry-After` (1 second, section 6.4) plus jitter, then retry. At most 5 times. After 6.3 a transfer refusal means the wait timed out, so it is rare |
| 429 with `reason` `request_budget` | No retry. The budget refills over a minute, and a render should not stall that long |
| Network error or 5xx from our server | Retry after 0.5, 1 and 2 seconds, with jitter |
| 403 or 404 from our server, or a digest mismatch | No retry |

When retries run out the asset is `missing`, and the render's existing behaviour applies (`preview-render.js:260-263`). A render that is superseded while it waits abandons its requests, as it abandons everything else.

### 5.3 The persistent store

- **Where:** a new IndexedDB database, `librepaper-assets-v1`, with one object store keyed by digest. Its name is added to `DATABASES` in `local-data.js`, so sign-out deletes it with the other two and no new clearing path is written.
- **Signed-out tabs:** the store checks `localDataForgotten()` before it opens the database, as `preview-cache.js` does.
- **Verification:** every body is checked against its digest before it enters the in-memory map, whether it came from the network or from the database. An entry that fails is deleted and fetched again. Bytes are written to the database only after they pass.
- **Size:** at most 256 MiB, evicted least recently used. An entry's last use is written at most once per tab. An asset larger than the cap is not stored.
- **When storage is unavailable:** in a private window, or when a write is refused, the store keeps today's in-memory map and nothing else. Nothing here throws.

The key is the digest alone, so an asset that two documents share is fetched once. That leaks nothing. A page only asks for the digests in a tree it could read, and the server would serve those bytes to the same reader. The server's access check, and `references_asset` for readers below Editor, run on every link request and every byte-route fetch.

The in-memory map stays as the first level, with its present key.

## 6. Serving assets

Phase 1.

The only part of an asset read that needs our server is the access check. Where the bytes sit in object storage, the server checks access and hands out a short-lived link, and the bucket serves the bytes. Where they sit on the server's disk, the server keeps relaying them, and a burst waits its turn instead of being refused.

### 6.1 The byte route

`GET` and `HEAD /api/documents/{slug}/assets/{sha}` stay as they are: same access check, same bytes, same headers (`private, no-store`, for the reasons in 3.5). The CLI's sync client reads and probes assets through it (`crates/librepaper-sync/src/client.rs:335-360`), tabs opened before the deploy use it, and deployments without object storage use nothing else. The comment at `figures.rs:144` is corrected to say the bytes are not cached, and why.

It is not turned into a redirect. A link is signed for one method, so the sync client's `HEAD` would fail against a link signed for `GET`. A redirect would also carry the browser's custom headers to the bucket and force a CORS preflight there.

### 6.2 Signed links

`GET /api/documents/{slug}/assets/{sha}/link`:

1. **The same checks as the byte route**: valid slug and digest, cross-site refusal, read access, `references_asset` for readers below Editor, the catalogue lookup scoped to the document. It shares that code with `handle_asset_read`, so the two cannot drift.
2. **Asks the blob store for a link.** `BlobStore` gains `signed_get(key, ttl) -> BlobResult<Option<Url>>`, `None` by default. `ObjectBlobStore` signs the link itself, with SigV4 query signing over the bucket's endpoint, region and keys, and two response overrides in the signed query: `response-content-type=application/octet-stream` and `response-content-disposition=attachment`. `FsStore` keeps the default.

   It does not use `object_store`'s `Signer::signed_url` (0.14, `aws/mod.rs:143`): that takes only a method, a path and a lifetime, and the signer that could sign extra query parameters (`AwsAuthorizer::sign`) is crate-private. SigV4 query signing is some 60 lines over `hmac` and `sha2`, both already dependencies, and is tested against AWS's published signing examples. The probe of 2026-11-13 (13.6) signed links this way against the production bucket.
3. **Answers** `200 {"url": "...", "expires_in": 300}`, or `200 {"url": null}` when the store cannot sign. `cache-control: private, no-store`.

Rules for the link:

- **Lifetime: 300 seconds.** Long enough to cover a slow fetch started at once, short enough that a person whose access is removed loses the bytes within five minutes. A link already issued stays valid until it expires; that window is the one thing this design gives up against the byte route, which refuses a revoked reader at the next request. A transfer that started before expiry may finish after it.
- **Expiry is enforced by the bucket.** `X-Amz-Expires` and `X-Amz-Date` are part of the signed string, so a link whose expiry is edited fails its signature. Rotating the access key kills every outstanding link at once.
- **No transfer permit.** The link route does no byte work. The transfer check in `cost.rs:509-511` matches `/assets/` anywhere in an `/api/` path, so it must exclude paths that end in `/link`.
- **The bytes stay untrusted.** The browser verifies every body against its digest (5.1), so the bucket cannot substitute bytes.
- **Opening a link directly downloads it.** The signed overrides make the bucket answer `application/octet-stream` and `attachment` whatever type the object was stored with, which covers every object already stored without one (3.4). `content-security-policy: sandbox` cannot be set by a bucket, and is not needed for a download; and a link that did render would render on the bucket's origin, not the app's. The overrides are inside the signature, so a reader cannot strip them.

What a deployment needs:

- **A CORS rule on the bucket** allowing `GET` from the app's origin. Without it the fetch in 5.1 fails as a network error and the store falls back to the byte route, so a self-hosted deployment on S3 keeps working without the rule, only slower. The production rule is set by `tools/deploy`; the self-hosting notes describe it.
- **Nothing in the app's CSP.** `SHELL_POLICY` (`reply.rs:369`) does not restrict connections.
- **A server clock in sync.** A signature dated outside the bucket's tolerance is refused.

What it costs on OVH (price page, 2026-11-13): Object Storage Standard bills no outgoing traffic, internal or external, and no requests. Moving bytes from the VPS to the bucket changes no bill and takes the bytes off the VPS's link.

What it changes for privacy: the bucket's operator sees the reader's address and the time of each fetch, which today it sees only as our server. `docs/privacy.md` is updated to say so.

### 6.3 Relayed bytes wait for a permit

Where the server relays bytes (the byte route, and `/published/` and `/assets/`), a request that finds no transfer permit free waits for one instead of being refused:

- **Admitted first, then waiting without a work permit.** Authentication and the request budget run before the wait, so the budget counts every request that may wait. For the wait itself the request gives its work permit back, and takes one again once it holds a transfer permit. Waiting with a work permit held would let waiting requests starve unrelated routes; waiting before the budget would let requests queue without being counted (found by review, 2026-11-13).
- **First come, first served.** `tokio::sync::Semaphore` hands permits out in order.
- **A deadline, not a cap.** A request that has waited 10 seconds is refused with `transfer_concurrency`. The deadline is a policy about how long a page may wait for one file, not a guess at anybody's capacity.
- **Bounded without another limit.** The deadline bounds how long each waiter lives, and the request budget (6000 a minute per principal or network), charged before the wait, bounds how many arrive.

One large project's burst now delays another reader's request by the time it takes to drain, instead of getting it refused. A fair share per principal would remove even that, and stays deferred (10.4).

### 6.4 Refusals say how long to wait

`refusal` (`cost.rs:224-232`) takes the wait as an argument:

| Reason | `retry-after` and `retry_after` |
|---|---|
| `transfer_concurrency`, `work_concurrency` | 1 |
| `request_budget`, `request_memory` | 60, as today |

Nothing reads either value today: no client in `web/src` or `skills/` looks at `Retry-After`. The store in 5.2 is the first.

### 6.5 No per-principal share

The first draft put a share of 8 concurrent transfers per principal in front of the deployment's semaphore. It is not in this design (section 10.4).

## 7. Staging that sends only what changed

Phase 2. Nothing in this phase changes what a renderer is given. The input is the same tree, byte for byte. Only the work of getting it there changes.

### 7.1 The profile comes first

Build the fixture of section 9 and time one file switch and one keystroke render for each of Markdown, Typst and LaTeX, split into these steps:

1. `tree()` and `previewTarget`.
2. Each call of `snapshotDigest`.
3. `gather`.
4. The message to the worker.
5. The hand-over into the module or engine.
6. The compile.
7. Publishing to the frame.

The table goes in the pull request. A candidate from 7.2 ships only if its step costs at least 50 ms on the fixture.

### 7.2 Candidates

In the order they are expected to matter:

1. **Text hashes are remembered.** `snapshotDigest` keeps the last text and digest per path and hashes a text again only when it differs. The function's output does not change, so it still matches the shared projection fixtures (`SPEC-unification.md`).
2. **The hand-over is incremental.** The page remembers what each renderer worker holds and sends only the entries that differ. The worker adds those with `add_file`, which replaces a file of the same name (`wasm-helpers/src/abi.rs:165-177`). When a path has gone, or the worker is new, the worker calls `clear_files` and adds everything from its own copy. Packages that `needs.js` fetched are added back as they are today.
3. **Data: URLs are made when asked for.** The store encodes a URL the first time a caller wants one for that asset, not for every asset it holds.
4. **Include lookups are remembered per text.** `includersOf` keeps each text's list of names and parses a text again only when it differs.

### 7.2a What the profile showed (2026-11-13)

A project shaped like PR 94's (704 Markdown notes, 2 MiB of text; 84 figures, 12 MiB), timed in Node on `dev` for one render after a one-character edit:

| Step | Before | After | Change |
|---|---|---|---|
| `session.tree()` | 25.6 ms | 8.6 ms | Text bodies re-read only for texts an event names (`bodyCache`); `compareUtf8` compares code points without encoding; path collisions checked through a prefix set instead of a scan of every taken path |
| `previewTarget` | 1.6 ms | 1.8 ms | Unchanged: under the line |
| `snapshotDigest`, per call (up to three per render) | 24 ms | 2.5 ms | Each path remembers its last text and digest |
| Message to the worker | 32 ms | 0.7 ms | The worker keeps the render tree; the page sends what changed (`treeDelta`, `applyTree`) |
| Hand-over into the module | 25–29 ms | 1.4 ms | `add_file` only for what changed; a removed path still clears and re-adds (`handOverChanged`) |
| Compile of the note | 0.1 ms | 0.1 ms | |

Outputs are unchanged: the digest and projection tests, the shared projection fixtures, and a comparison of the old and new `projectDirectory` on 800 random projects agree. Completion requests keep sending a whole tree of their own, outside the worker's copy.

### 7.3 LaTeX staging is left alone

LaTeX flushes and rewrites its file systems on every compile because `flushCache` is the only way to remove a file, and a stale `.aux` left behind can break the next compile (`latex/worker.js:318-330`). Writing only what changed would need an engine command that removes one file, which is a change to `wasm-latex`. It is proposed only if the profile shows LaTeX staging above the 50 ms line, and then as its own decision. Measured in Chromium on the same project with pdfLaTeX: staging takes 108 ms per compile (0.8 ms for a two-file project), against 1.6 s for one TeX pass. It is above the line and about 7% of a compile, so it is left for that separate decision.

### 7.4 Identity

Unchanged. The identity is `snapshotDigest` of the whole tree. Candidate 1 makes it cheap. It does not make it narrower (section 10.3).

## 8. A project is the unit of loading

Phase 3. One path for every format: when a project opens, the browser fetches all of its assets in the background, and keeps fetching whatever the project gains while it is open. A render, a file switch, a new tab or a reopened project then finds the bytes local, whatever the format.

### 8.1 Why whole projects

The alternatives are per format, and no single one covers every renderer:

- **Render first, fetch what the output names** works for the four flow-page renderers, which write an asset's URL and never read its bytes. It cannot work for LaTeX or Typst, which read an image's bytes to lay out the page (`\includegraphics` needs its size), or answer `\IfFileExists` without failing.
- **Fetch as the engine reads** works for every renderer in principle, but engine reads are synchronous, so it needs blocking network reads in workers, a synchronous local store, a host callback in three engines, and serial fetches on a cold open (10.2).

Loading the project whole needs none of that, gives every renderer the complete tree with nothing withheld (rule 1), and costs, after phase 1, one fetch per asset per browser until sign-out.

### 8.2 Warm on open

- **When:** whenever the reader refreshes its file list, which it does once the project is restored and after every batch that changes the directory (`refreshFiles`, `Reader.svelte`). A change to an asset's bytes is a change to its digest, so a replaced, added or renamed figure arrives through the same call.
- **What:** `figures.gather` over every digest in the current tree, in the background, through the phase 1 store (memory, the persistent store, a signed link, the byte route). Digests already held cost nothing. Requests for the same digest are merged with the render's own.
- **Renders are unchanged.** A render still gathers its tree's assets before it compiles, as every render does today. On a cold open it waits for the same fetches the warm-up started; afterwards it finds everything in memory.
- **Updates midway** need nothing more. An old version still downloading finishes under its old digest and is never used: the next render asks for the new digest. A deleted asset that was still downloading is either refused or stored and never used.

### 8.3 The store keeps the open project

The persistent store evicts least recently used entries to stay under its cap (5.3). A project's assets must not evict each other while it is open, or a file switch would fetch again:

- The reader tells the store which digests the open project holds (`figures.keep`), every time it warms.
- Eviction skips those digests. If the store cannot get under its cap without them, the entry just written is removed instead: a project bigger than the cap keeps what fits, and the rest comes from the network in the next cold tab. Within one tab, the in-memory map holds the whole project regardless.
- Leaving the document (`release`) clears the kept set.

### 8.4 What it costs

The first open of a project in a browser downloads every asset, including figures nobody looks at in that session. On a normal connection that is one parallel burst from the bucket; on a slow connection or a very figure-heavy project it is the time to first page that grows (13.8). Later opens, new tabs and file switches cost no network.

## 9. Testing

- **`figures.mjs` (unit):**
  - an asset is fetched through its link when the server gives one;
  - a `null` link, or a link fetch that fails as a network error, falls back to the byte route, and the tab then skips the link request;
  - a 403 from the bucket asks for one new link, then falls back;
  - a 429 for `transfer_concurrency` is retried after `Retry-After`, and five failures give `missing`;
  - a 429 for `request_budget` is not retried;
  - a digest mismatch is not retried;
  - a stored entry that fails its digest is deleted and fetched again;
  - nothing is written after `localDataForgotten()`;
  - eviction keeps the store under its cap;
  - with no IndexedDB, the store still fetches.
- **`local-data` (unit):** `forgetLocalData` deletes `librepaper-assets-v1`.
- **Server:**
  - the link route refuses exactly what the byte route refuses: no access, a digest outside the current tree for a reader below Editor, another document's digest;
  - on a file store it answers `{"url": null}`; on an object store, a link whose query carries `X-Amz-Expires=300` and a signature;
  - it takes no transfer permit;
  - with every transfer permit held, a byte-route request waits and succeeds when one frees, and is refused with `transfer_concurrency` after the deadline;
  - a waiting request holds no work permit;
  - concurrency refusals carry `retry-after: 1`, in the header and the body;
  - budget refusals carry 60;
  - asset reads still carry `private, no-store`.
- **Bucket (manual, once per provider):** a signed `GET` link works, an expired one and one with an edited expiry are refused, a `HEAD` on a `GET` link is refused, the response overrides are honoured, and the CORS rule admits the app's origin. See 13.6.
- **Staging (unit):**
  - `snapshotDigest` gives the same digest with and without its memory, on the shared projection fixtures;
  - an incremental hand-over leaves the module holding the same files as a full one after an edit, an added file, a removed file and a rename;
  - a new worker receives everything.
- **Project loading (unit, `figures.mjs`):**
  - eviction never removes a kept digest; another project's oldest entries go first;
  - when kept digests alone exceed the cap, the entry just written is not left in the store;
  - `release` clears the kept set.
- **Browser:** a fixture project with 700 notes, 80 figures and one LaTeX paper that uses 10 of them.
  - Opening the project requests each asset once, in the background, whichever file is open.
  - Switching files afterwards requests no asset, in every format.
  - Adding or replacing a figure requests that one asset.
  - A second open in a new tab requests none.
  - Opening the paper requests each asset once, and none is refused, on a server started with a file store and on one started with an S3 emulator.
  - After sign-out, the asset database is gone, and a second account that opens the same document fetches its assets from the server.
- **Benchmark:** on the same fixture, record asset requests on open, time to first page, and time for a file switch, before phase 1 and after each phase. These are the numbers PR 94 reports.

## 10. Deferred, and what would reopen each

### 10.1 A scope built from project structure

PR 94's allow-list, and the first draft's inverse of it with a fall-back to the whole tree. Not built, because of rule 1 and section 3.9: a structural scope withholds files on a guess, and the failures it causes are often silent. Phases 1 and 3 remove the 429s and the repeated fetches PR 94 measured without it.

Nothing reopens this in that form. A narrower input is acceptable only when it is exact (10.2).

### 10.2 Loading files as a render reads them

The one other path that would treat every format alike: each engine reads project files through one interface, the file list comes complete from the tree, and bytes are fetched at the moment of the read. It is exact by construction and would not download figures nobody opens. It needs:

- a blocking read inside each renderer's worker (a synchronous `XMLHttpRequest` is allowed there, cross-origin with CORS);
- a synchronous local store, since IndexedDB cannot be read synchronously (the origin private file system's synchronous handles lock a file to one worker, and their shared read mode is not available in every browser);
- a host callback for a missed file in each of our three engines, and three engine releases;
- and it fetches a cold compile's figures one after another.

Not built, because loading the project whole (8) gets every format the same result after the first open, with none of this.

**Reopen when** projects are seen that are too large to load whole: a first open measured as slow because of figures the session never uses, or projects that routinely exceed the store's cap.

### 10.3 A render identity over the files a render read

It would stop an edit to an unrelated file from invalidating a finished page. It needs a complete read set, and none exists: BibTeX and Biber read `.bib` files that TeX's recorder never lists, a file that was looked for and not found is a dependency too, and what LaTeXML reports is unknown (3.11). A page kept under an incomplete read set is a stale page shown as current.

**Reopen when** the profile shows recompiles after unrelated edits are a cost people feel, and an engine can report every file it read and every file it looked for.

### 10.4 A per-principal share of transfers

A share of 8 would refuse the app's own bundles, which take the same permits and load some 75 at a time (3.4). And where permits are taken, a visitor without an account is known only by network (`cost.rs:94-99`), so a classroom behind one address would share 8. After 6.2, asset bytes on object storage take no permit at all, and after 6.3 a burst on the rest waits rather than fails.

**Reopen when** waits on the transfer pool are seen to delay other readers noticeably, and then as a fair queue with the bundles outside it and a principal that tells visitors apart.

### 10.5 A digest plan for desktop uploads

Section 3.10. **Reopen if** `MAX_UPLOAD_BYTES` is raised well past 64 MiB. Then record digests in the workspace's tracking file and let the app answer which files it lacks. The workspace is the store; a second one is not needed.

### 10.6 A batch endpoint

HTTP/2 already multiplexes requests, and the store removes most of them. A link costs one small request before each cold fetch. **Reopen if** the profile shows link requests on a cold open of a figure-heavy project as a cost people feel; then one request signs links for many digests.

## 11. Phases

Each phase ships alone and is useful alone.

| Phase | Content | Risk to correctness |
|---|---|---|
| 1 | Asset store (5); signed links, waiting for permits and refusals that say how long to wait (6) | None. Same inputs, fetched better. Every body is verified against its digest |
| 2 | Profile, then staging that sends only what changed (7) | None. The renderer's input is identical; the tests in 9 hold it to that |
| 3 | A project is the unit of loading: warm on open, keep the open project in the store (8) | None. Renders get the same tree; only when the bytes arrive changes |

Phase 1 alone removes the 429s a client causes itself and the refetch on every new tab. Phase 3 makes every file switch and every later open free of network, in every format; the first open of a project pays for all of its assets once.

## 12. What changed

### 12.1 From the second draft (2026-11-13)

| Second draft | Now | Why |
|---|---|---|
| Phase 3: flow pages render with placeholders, then fetch the assets the page names | The project is the unit of loading: every asset fetched in the background when it opens, the open project kept in the store (8) | Render-then-check worked for four renderers and not for LaTeX or Typst. One path for every format was wanted; whole projects are that path, and phase 1 had already built most of it |
| Demand loading for compiled renders, deferred | Loading files as a render reads them, deferred for every format (10.2) | Considered as the uniform alternative; it needs blocking reads, a synchronous store and three engine changes |
| At most 6 asset requests in flight per tab | No client cap | Six was a guess at a server-side capacity the client cannot see, and it would differ by deployment |
| The server relays every asset byte | A signed link to the bucket on object storage (6.2), the byte route kept for files on disk, the CLI and old tabs | The access check is the only part that needs our server. OVH bills no egress or requests, so the bucket's share costs nothing |
| A busy transfer pool refuses at once, with `retry-after: 1` | A request waits up to 10 seconds for a permit, transfer before work (6.3) | A permit frees in milliseconds; refusing turned ordinary bursts into errors. This also covers the app's bundles and tabs opened before the deploy (13.1, 13.3) |

### 12.2 From the first draft

| First draft | Now | Why |
|---|---|---|
| Asset bytes in Cache Storage, partitioned by account, 512 MiB; plus year-long HTTP caching | One IndexedDB database deleted at sign-out, 256 MiB; headers stay `no-store` | The HTTP cache outlives sign-out and cannot be cleared reliably (3.5). Deleting at sign-out makes a partition unnecessary, and the path that deletes IndexedDB already exists (3.6) |
| Section 3.5: `blob_response` sets no `Cache-Control` | It sets `private, no-store` itself (`cost.rs:713`) | Read of the code |
| A per-principal share of 8, with `retry-after: 1` | No share. All concurrency refusals say 1 second | 10.4. A permit frees in milliseconds whichever semaphore refused it |
| Render and prefetch priorities | One queue | Nothing prefetches once read sets are gone |
| Render scope from structure, with escalation on matching errors | Not built | 3.9 and 10.1. Several escalation triggers could never fire: authored HTML cannot fail, and a file outside the scope cannot appear in LaTeX's recorder |
| Quarto exceptions for `_quarto.yml` and `_extensions` | Dropped | The browser draft reads none of those files (3.2) |
| Render identity over the scope | Whole tree, hashed cheaply | 10.3 |
| Observed read sets | Deferred | 10.2 and 10.3. `inputFiles` is untested here and incomplete for identity |
| Content store and `jobs/plan` for the desktop app, wire 3.3.0 | Dropped | 3.10. No wire change is needed |
| Lazy URLs for Markdown, Quarto and Org | Render first, fetch what the output names, substitute; itself replaced by whole-project loading (12.1) | A blob: URL or the asset route cannot load in the documents frame (`figures.js:9-17`) |
| Lazy bytes for Typst through `set_asset_url` | Deferred (10.2) | Typst reads bytes from the file map; a URL does nothing for it |
| No measurement step | Phase 2 starts with a profile | Rule 4. Section 3.8 lists six per-render costs and nobody knows their sizes |

## 13. Open questions

1. **Do the app's bundles cause refusals?** They share the transfer pool with document assets (3.4). After 6.3 they wait instead. The benchmark should still record, from the server's metrics, how long requests wait for a transfer permit, by path.
2. **Store cap:** is 256 MiB right? The stand-in pages have 96 MiB. A person with several figure-heavy projects may want more; a phone less.
3. **Tabs opened before the deploy** keep the unbounded fetch on the byte route until they reload. After 6.3 their burst waits for permits instead of drawing 429s, so they need no change.
4. **The 50 ms line** in 7.1 is a proposal. The profile may show a better one.
5. **Authored HTML in nested frames:** a nested frame's own images are not resolved today, because the frame is inlined before the rewrite runs. Fixing it is a separate question.
6. **The production bucket's behaviour** (9, "Bucket"), probed 2026-11-13 against OVH `bhs` with links signed by a stand-alone SigV4 signer:
   - a signed `GET` link works; one used after expiry, one with an edited expiry, and a `HEAD` on a `GET` link are refused with 403;
   - the response overrides are honoured: `Content-Type: application/octet-stream`, `Content-Disposition: attachment`;
   - the bucket has no CORS configuration today, and answers a request with an `Origin` header without `Access-Control-Allow-Origin`. The rule in 6.2 has to be set before the store can use links; until then it falls back to the byte route;
   - what the bucket answers for an object stored with no type at all is not known: the probe's upload carried a type its HTTP library added. The signed overrides make it irrelevant.
7. **The 10-second wait** in 6.3 is a proposal. The benchmark's wait times may show a better one.
8. **The first open of a large project** downloads every asset. Measure time to first page on the fixture over a throttled connection, and on a phone, before deciding whether 10.2 is ever needed.
