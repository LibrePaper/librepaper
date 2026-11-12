# Render transport: getting a project to its renderers

Status: proposed 2026-11-11. This replaces the first draft (`e769c89ae`). Section 12 lists what changed and why.

A render reads a project: texts, figures, fonts, bibliographies, styles. Today every preview render collects the whole project, fetches every binary asset at once, and hands all of it to the renderer. On a large project that is slow, and the parallel fetches can exhaust the server's transfer permits, so the server answers 429 to page opens. This document specifies how assets are fetched and kept, how the work a render repeats is cut, and how a flow page fetches only the assets it shows.

PR 94 (`renderScope`, not merged) found the problem. On a project of 704 texts and 84 assets, a page open fetched all 84 assets and sometimes drew 429s, and a file switch was slow. The PR's fix hands the renderer an allow-list of files. It drops files that compilers do read (section 3.9). This spec keeps the PR's goal and reaches it without ever withholding a file on a guess.

## 1. Product principle

> A render reads the whole project. It pays only for what changed and for what the page shows.

Four rules follow from it:

1. **The renderer's input never shrinks on a guess.** No file is withheld because a heuristic judged it unrelated. Work is saved in two ways only: by not repeating it, and by exact knowledge of what the output uses.
2. **Bytes travel once where the browser can keep them.** An asset is addressed by its SHA-256. A browser that has fetched and stored it does not fetch it again until the person signs out. Three ordinary conditions fetch again and are accepted: an entry the store evicted, a browser that refuses storage, and two tabs that open cold at the same moment.
3. **Nothing private outlives sign-out.** Whatever this browser keeps of a project is deleted when the person signs out, the way projects and rendered pages already are (`web/src/lib/local-data.js`).
4. **Measure, then change.** Every phase after the first starts from a profile and changes only the steps the profile shows to matter.

## 2. Scope

### In scope

- One client asset store: bounded concurrency, retries, and a persistent store that sign-out deletes (section 5).
- Refusals that say how long to wait (section 6).
- A profile of one render, and staging that sends a renderer only what changed (section 7).
- Flow pages that fetch only the assets their output shows (section 8).

### Out of scope

- The room protocol (`librepaper.room.v3`) and how texts reach the browser. Texts are already local when a render starts.
- The asset mirror (`docs/dev/asset-mirrors.md`). It serves pinned engine modules, fonts and LaTeX release files, not project assets.
- Server-side renders, such as MCP candidate renders (`crates/librepaper-server/src/server/mcp/render.rs`).
- The desktop app's local renders. They keep the whole tree and today's upload (section 3.10).
- Everything in section 10: a scope built from project structure, demand loading for compiled renders, a render identity over read files, a per-principal share of transfers, a digest plan for desktop uploads, and a batch endpoint. Each is deferred with the evidence that would reopen it.

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

- **Transfer permits:** `DEFAULT_ARTIFACT_TRANSFERS = 64` (`crates/librepaper-base/src/config/mod.rs:183`) is one semaphore for the whole deployment. It covers `/api/.../assets/`, and also `/published/` and `/assets/`, the web app's own bundles (`cost.rs:509-511`). A permit is held until the body finishes streaming.
- **Work permits:** `work_concurrency` is 64 as well, taken for every request and held until the handler answers.
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
- **Placeholder:** a stand-in URL for an asset whose bytes are not in memory yet (section 8.2).

## 5. The client asset store

Phase 1. `web/src/lib/figures.js` becomes the asset store. Every caller that needs asset bytes goes through it: previews, candidate previews, exports, downloads, workspace previews and `figureUrlFor`.

### 5.1 Bounded fetching

- **At most 6 requests in flight per tab** (`ASSET_CONCURRENCY = 6`), in a first-in, first-out queue. Six matches a browser's HTTP/1.1 per-origin limit, so nothing is lost without HTTP/2, and it is well below the deployment's 64 permits.
- **One queue for all callers**, so a preview and an export started together do not double the load.

### 5.2 Retries

| Response | What the store does |
|---|---|
| 429 with `reason` `transfer_concurrency` or `work_concurrency` | Wait for `Retry-After` (1 second after section 6.2) plus jitter, then retry. At most 5 times |
| 429 with `reason` `request_budget` | No retry. The budget refills over a minute, and a render should not stall that long |
| Network error or 5xx | Retry after 0.5, 1 and 2 seconds, with jitter |
| 403, 404 or a digest mismatch | No retry |

When retries run out the asset is `missing`, and the render's existing behaviour applies (`preview-render.js:260-263`). A render that is superseded while it waits abandons its requests, as it abandons everything else.

### 5.3 The persistent store

- **Where:** a new IndexedDB database, `librepaper-assets-v1`, with one object store keyed by digest. Its name is added to `DATABASES` in `local-data.js`, so sign-out deletes it with the other two and no new clearing path is written.
- **Signed-out tabs:** the store checks `localDataForgotten()` before it opens the database, as `preview-cache.js` does.
- **Verification:** every body is checked against its digest before it enters the in-memory map, whether it came from the network or from the database. An entry that fails is deleted and fetched again. Bytes are written to the database only after they pass.
- **Size:** at most 256 MiB, evicted least recently used. An entry's last use is written at most once per tab. An asset larger than the cap is not stored.
- **When storage is unavailable:** in a private window, or when a write is refused, the store keeps today's in-memory map and nothing else. Nothing here throws.

The key is the digest alone, so an asset that two documents share is fetched once. That leaks nothing. A page only asks for the digests in a tree it could read, and the server would serve those bytes to the same reader. The server's `references_asset` check for readers below Editor still runs on every fetch that reaches it.

The in-memory map stays as the first level, with its present key.

## 6. Serving assets

Phase 1.

### 6.1 Headers

Unchanged: `private, no-store`, for the reasons in 3.5. The comment at `figures.rs:144` is corrected to say so and to say why.

### 6.2 Refusals say how long to wait

`refusal` (`cost.rs:224-232`) takes the wait as an argument:

| Reason | `retry-after` and `retry_after` |
|---|---|
| `transfer_concurrency`, `work_concurrency` | 1 |
| `request_budget`, `request_memory` | 60, as today |

Nothing reads either value today: no client in `web/src` or `skills/` looks at `Retry-After`. The store in 5.2 is the first.

### 6.3 No per-principal share

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

### 7.3 LaTeX staging is left alone

LaTeX flushes and rewrites its file systems on every compile because `flushCache` is the only way to remove a file, and a stale `.aux` left behind can break the next compile (`latex/worker.js:318-330`). Writing only what changed would need an engine command that removes one file, which is a change to `wasm-latex`. It is proposed only if the profile shows LaTeX staging above the 50 ms line, and then as its own decision.

### 7.4 Identity

Unchanged. The identity is `snapshotDigest` of the whole tree. Candidate 1 makes it cheap. It does not make it narrower (section 10.3).

## 8. Flow pages fetch what they show

Phase 3. A flow page's renderer never reads asset bytes. It writes URLs. So the page can be rendered first and its assets fetched afterwards, and only the ones it shows.

### 8.1 Which renders

A preview render whose renderer is Markdown, Org, the Quarto draft or authored HTML, producing HTML in this browser. Every compiled render keeps today's order: gather everything, then render.

Only `preview-render.js` changes. Candidate previews, label comparisons, exports and downloads gather every asset, as today.

### 8.2 The sequence

1. **Build the URL map without fetching.** The map has the same keys as today, one per path in `digests`. An asset already in memory gets its real URL. Any other gets a placeholder: `librepaper-pending-asset/<sha256>`.
2. **Render.** The renderer receives no asset bytes.
3. **Read the output.** Collect every placeholder in the HTML. Those digests are exactly the assets the page shows that are not in memory.
4. **Fetch them** through the store.
5. **Substitute.** Each placeholder is replaced by the asset's data: URL, with the `#librepaper-asset=<sha>` fragment the documents agent reads.
6. **Publish once.** The frame is given the finished page, never one with holes. The render's guards against a newer ticket, a navigation and a source change are checked again after the fetch, like after every other await.

The stand-in page store and the `finished` map keep the page as published, with its data: URLs.

### 8.3 An asset that cannot be fetched

Its placeholder is replaced by an empty data: URL with the same fragment. The image is broken in the page, which is what a missing figure in a flow page looks like today, and the frame requests nothing from the server for it.

### 8.4 Why this is exact

Each of the four renderers resolves every asset reference through the URL map and reads no asset any other way (3.2). The map has the same keys as before, so the renderer makes the same decisions. A placeholder is a fixed prefix and 64 hexadecimal digits, which HTML and URL escaping leave alone. The page that results is the page a full gather would have produced.

That claim is tested, not assumed (section 9): for each renderer, the page built through placeholders must equal, byte for byte, the page built with every asset gathered first.

## 9. Testing

- **`figures.mjs` (unit):**
  - at most 6 requests in flight;
  - a 429 for `transfer_concurrency` is retried after `Retry-After`, and five failures give `missing`;
  - a 429 for `request_budget` is not retried;
  - a digest mismatch is not retried;
  - a stored entry that fails its digest is deleted and fetched again;
  - nothing is written after `localDataForgotten()`;
  - eviction keeps the store under its cap;
  - with no IndexedDB, the store still fetches.
- **`local-data` (unit):** `forgetLocalData` deletes `librepaper-assets-v1`.
- **Server:**
  - concurrency refusals carry `retry-after: 1`, in the header and the body;
  - budget refusals carry 60;
  - asset reads still carry `private, no-store`.
- **Staging (unit):**
  - `snapshotDigest` gives the same digest with and without its memory, on the shared projection fixtures;
  - an incremental hand-over leaves the module holding the same files as a full one after an edit, an added file, a removed file and a rename;
  - a new worker receives everything.
- **Flow pages (unit, one set per renderer):** the page built through placeholders equals the page built with every asset gathered, on documents with:
  - a percent-encoded name (`my%20plot.png`);
  - an image written as raw HTML inside Markdown;
  - an image inside an included file;
  - an asset the document never names, which must not be fetched;
  - an asset that cannot be fetched, which becomes a broken image and not an error.
- **Browser:** a fixture project with 700 notes, 80 figures and one LaTeX paper that uses 10 of them.
  - Opening a note with no figure requests no asset.
  - Opening a note with two figures requests two.
  - A second open in a new tab requests none.
  - Opening the paper requests each asset once, never more than 6 at a time, and none is refused.
  - After sign-out, the asset database is gone, and a second account that opens the same document fetches its assets from the server.
- **Benchmark:** on the same fixture, record asset requests on open, time to first page, and time for a file switch, before phase 1 and after each phase. These are the numbers PR 94 reports.

## 10. Deferred, and what would reopen each

### 10.1 A scope built from project structure

PR 94's allow-list, and the first draft's inverse of it with a fall-back to the whole tree. Not built, because of rule 1 and section 3.9: a structural scope withholds files on a guess, and the failures it causes are often silent. Phases 2 and 3 reach PR 94's results for the project it measured without it.

Nothing reopens this in that form. A narrower input is acceptable only when it is exact, as in section 8 and 10.2.

### 10.2 Demand loading for compiled renders

Compiled renders still gather every asset. After phase 1 that costs one fetch per asset per browser, until sign-out.

- **Typst:** the module could report project files it did not find, beside the packages it already reports. The place is one line (`wasm-typst/src/typst.rs:321`), and `needs.js` already has the fetch-and-compile-again loop. But Typst stops at the first file it cannot read, so a cold render would take one round per figure.
- **LaTeX:** the resolver evidence stream could name the project path a compile looked for and did not find. It would have to be shown first that every such miss reaches the stream, including `\IfFileExists` and `\includegraphics` without an extension.

**Reopen when** a measured cold open of a compiled document is slow because it fetches assets the page does not use, on a project where that is most of them.

### 10.3 A render identity over the files a render read

It would stop an edit to an unrelated file from invalidating a finished page. It needs a complete read set, and none exists: BibTeX and Biber read `.bib` files that TeX's recorder never lists, a file that was looked for and not found is a dependency too, and what LaTeXML reports is unknown (3.11). A page kept under an incomplete read set is a stale page shown as current.

**Reopen when** the profile shows recompiles after unrelated edits are a cost people feel, and an engine can report every file it read and every file it looked for.

### 10.4 A per-principal share of transfers

A share of 8 would refuse the app's own bundles, which take the same permits and load some 75 at a time (3.4). Two tabs at 6 requests each would exceed it. And where permits are taken, a visitor without an account is known only by network (`cost.rs:94-99`), so a classroom behind one address would share 8.

**Reopen when** a single client is seen to hold the pool, and then with the bundles outside it and a principal that tells visitors apart.

### 10.5 A digest plan for desktop uploads

Section 3.10. **Reopen if** `MAX_UPLOAD_BYTES` is raised well past 64 MiB. Then record digests in the workspace's tracking file and let the app answer which files it lacks. The workspace is the store; a second one is not needed.

### 10.6 A batch endpoint

HTTP/2 already multiplexes requests, and a bounded client with a store removes most of them. **Reopen if** a cold open of a figure-heavy compiled project is still slow after phase 1.

## 11. Phases

Each phase ships alone and is useful alone.

| Phase | Content | Risk to correctness |
|---|---|---|
| 1 | Asset store (5), refusals that say how long to wait (6) | None. Same inputs, fetched better |
| 2 | Profile, then staging that sends only what changed (7) | None. The renderer's input is identical; the tests in 9 hold it to that |
| 3 | Flow pages fetch what they show (8) | None by construction, and held to byte equality by the tests in 9 |

Phase 1 alone removes the 429s a client causes itself and the refetch on every new tab. Phase 3 is what turns PR 94's 84 requests into none for a note without figures. Its author could be offered phase 3: their project is the benchmark.

## 12. What changed from the first draft

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
| Lazy URLs for Markdown, Quarto and Org | Render first, fetch what the output names, substitute (8) | A blob: URL or the asset route cannot load in the documents frame (`figures.js:9-17`) |
| Lazy bytes for Typst through `set_asset_url` | Deferred (10.2) | Typst reads bytes from the file map; a URL does nothing for it |
| No measurement step | Phase 2 starts with a profile | Rule 4. Section 3.8 lists six per-render costs and nobody knows their sizes |

## 13. Open questions

1. **Do the app's bundles cause refusals?** They share the transfer pool with document assets (3.4). Phase 1 should check, from the server's logs on the benchmark, which paths `transfer_concurrency` refuses before and after. If bundles are among them, taking them out of the pool is a separate, small change.
2. **Store cap:** is 256 MiB right? The stand-in pages have 96 MiB. A person with several figure-heavy projects may want more; a phone less.
3. **Tabs opened before the deploy** keep the unbounded fetch until they reload, and they do not retry. This is accepted: the server would have to wait for a permit on their behalf, and that changes how every refusal works for a problem that ends at the next reload.
4. **The 50 ms line** in 7.1 is a proposal. The profile may show a better one.
5. **Authored HTML in nested frames:** a nested frame's own images are not resolved today, because the frame is inlined before the rewrite runs. Section 8 keeps that behaviour. Fixing it is a separate question.
