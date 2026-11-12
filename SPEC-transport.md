# Render transport: getting a project to its renderers

Status: proposed 2026-11-12

A render reads a project: texts, figures, fonts, bibliographies, styles. Today every preview render collects the whole project, fetches every binary asset at once, and hands all of it to the renderer. On a large project that is slow, and the parallel fetches can exhaust the server's transfer permits, so the server answers 429 to page opens. This document specifies how a render obtains what it needs: how assets are fetched and cached, how a render is scoped to the files it reads, and how the scope stays correct when a guess would be wrong.

PR 94 (`renderScope`, not merged) found the problem and measured it. On a project of 704 texts and 84 assets, a page open fetched all 84 assets and sometimes drew 429s, and a file switch waited on rendering the whole corpus. The PR's fix keeps an allow-list of files reached by includes, plus support files picked by extension. It drops files that compilers do read (section 3.7). This spec keeps the PR's goal and replaces the allow-list with a design that cannot make a render fail that works today.

## 1. Product principle

> A render costs what the page reads, and never fails for want of a file the project has.

Three rules follow from it:

1. **Bytes travel once.** An asset is addressed by its SHA-256. A browser that has fetched it once never fetches it again, whatever document or render asks next.
2. **Scoping saves work but never decides correctness.** When the scope might be wrong, the render falls back to the whole project. A narrower scope is a speed-up that has to prove itself, not a rule the person can trip over.
3. **Exports and downloads take everything.** A file someone downloads is the whole project. Only previews are scoped.

## 2. Scope

### In scope

- One client asset store: bounded concurrency, retries that honour `Retry-After`, a persistent cache keyed by digest, partitioned by account (section 5).
- Cache headers on asset reads that match their content addressing, and a per-principal share of the transfer permits (section 6).
- A render scope built from project structure, with a fall-back to the full tree when a render fails in a way a missing file could explain (section 7).
- A render identity computed over the scoped inputs, so editing an unrelated file does not invalidate a cached page (section 8).
- Observed read sets: renderers report the project files a render opened, and the next render of the same page starts from that set (section 9).
- A content-addressed upload to the desktop app's local renders, so a job sends only the bytes the app lacks (section 10).
- Lazy materialisation for engines that can fetch on demand (section 11).

### Out of scope

- The room protocol (`librepaper.room.v3`) and how texts reach the browser. Texts are already local when a render starts.
- The asset mirror (`docs/dev/asset-mirrors.md`). It serves pinned engine modules, fonts and LaTeX release files, not project assets. It stays as it is.
- Server-side renders, such as MCP candidate renders (`crates/librepaper-server/src/server/mcp/render.rs`).
- A batch endpoint that returns several assets in one response. HTTP/2 already multiplexes requests, so a bounded client and a cache remove the need (section 13).

## 3. Current behavior and constraints

### 3.1 The tree

The preview builds one tree per render and reads it once (`web/src/lib/reader/preview-render.js:148`).

- `session.tree()` (`web/src/lib/project-session.js:915-952`) returns `{ texts, digests, files, retained }` for the whole directory. It is cached on the Loro oplog version plus the projection rules.
- Without a session, `projection.tree()` (`web/src/lib/reader/projection-source.svelte.js:141-155`) returns `{ main: "", texts, retained, digests, settings }`.
- `Reader.svelte` overlays the editor's live text (`liveTreeNow`, 1860-1871) and adds `main: previewPath` (`treeNow`, 1876-1880). `previewPath` comes from `previewTarget` in `web/src/lib/opening.js:290-312`, with reasons `locked`, `alone`, `inside` or `none`.

`digests` maps every binary asset's path to its SHA-256. `texts` holds every text.

### 3.2 What each renderer receives

The dispatcher's comment says outright that `tree` is "the whole directory" (`web/src/lib/renderers.js:186-189`). Every renderer gets every text and every gathered asset by value:

| Renderer | How it receives the tree | Where |
|---|---|---|
| LaTeX to PDF (wasm) | Worker `stage` message. `writeTree` writes every text and asset into each engine's Emscripten FS | `latex.js:733`, `latex/worker.js:308-337, 597-601` |
| LaTeX to HTML (LaTeXML) | `{...tree.texts, ...tree.assets}` written the same way | `latex/html.js:254-260` |
| Typst, Markdown, Quarto, Org | `renderTree` copied to a worker. `handOver` calls `clear_files`, then `add_file` for each file, `set_asset_url`, `set_main` | `renderers.js:325-336`, `renderer-wasm.js:150-167` |
| Authored HTML | `resolveHtmlAssetUrls` rewrites `src`, `poster` and `data` to data: URLs, and inlines nested frames | `renderers.js:90-120` |
| Desktop app (protocol 3) | Multipart POST of every text and asset with a `{path, sha256, size}` manifest to `librepaper/local/jobs` | `desktop/client.js:418-430, 515-547` |
| The docs frame | Output only, by postMessage: HTML with data: URLs, or PDF bytes | `reader/frame-preview.js:87-89` |

### 3.3 Fetching assets

`gather` (`web/src/lib/figures.js:89-136`) runs an unbounded `Promise.all` over every entry in `digests`.

- `fetchOne` (33-58) requests `/api/documents/{slug}/assets/{sha}` with `cache: "no-store"` and verifies the SHA-256.
- The only cache is an in-memory `Map` keyed by scope and digest, which also merges in-flight requests. It dies with the tab.
- There is no retry. A failed fetch becomes `missing`, and paged output and LaTeX HTML then throw (`preview-render.js:260-263`).

### 3.4 Serving assets

`handle_asset_read` (`crates/librepaper-server/src/server/figures.rs:147-222`) checks read access. Readers below Editor also need `room.references_asset(sha)`. It then streams through `cost::blob_response` (`cost.rs:629`).

- **Transfer permits:** `DEFAULT_ARTIFACT_TRANSFERS = 64` (`crates/librepaper-base/src/config/mod.rs:183`) is one semaphore for the whole deployment, covering every `/api/.../assets/` path. A permit is held until the body finishes streaming.
- **Work permits:** `work_concurrency` is 64 as well, a semaphore on every request.
- **Request budget:** `requests_per_principal_minute` is 6000.
- **Refusals:** all three answer 429 with `retry-after: 60` and a `reason` of `transfer_concurrency`, `work_concurrency` or `request_budget` (`cost.rs:224-232`).
- **Why one client hits it:** a client that opens 84 fetches at once takes more than the deployment's 64 transfer permits.

### 3.5 Cache headers contradict the addressing

The comment on `handle_asset_read` says the bytes are "cached for a year" (`figures.rs:144`). They are not: `default_cache_control` (`cost.rs:559-568`) marks every response `private, no-store` unless it carries both an `ETag` and its own `Cache-Control`. `blob_response` sets the `ETag` but no `Cache-Control`. The client also asks for `no-store`. In practice no asset is ever cached by HTTP, so every page open in a new tab refetches every asset.

### 3.6 Render identity and caches

- **Snapshot digest:** `snapshotDigest(tree)` (`web/src/lib/projection-digest.js:75-89`) hashes `main` and every file in the project. Editing any file, even one the page never reads, changes the identity.
- **Finished renders:** a match in the in-memory `finished` map (`preview-render.js:102-113`, 8 entries) skips the compile and the fetch. Its key includes that identity (208).
- **Stand-in page:** the IndexedDB page kept for a stand-in (`reader/preview-cache.js`) is keyed by slug and path, not identity. It is a placeholder, not a cache hit.
- **LaTeX results:** a LaTeX result is accepted only if the identity still matches (`preview-render.js:307-309`).

### 3.7 Why an allow-list breaks renders

`includesOf` (`web/src/lib/opening.js:213`) follows these, and nothing else:
- LaTeX: `\input`, `\include`, `\subfile`
- Typst: `#include`, `#import`
- Org: `#+INCLUDE`, `#+SETUPFILE`
- Quarto and Markdown: `{{< include >}}`

A scope built from those includes plus an extension list misses files that renders read:

| Read | Format | Why it is missed |
|---|---|---|
| `css: style.css`, a `.scss` theme, `filters: [x.lua]`, template partials, `_extensions/**` | Quarto | Named in YAML, not included |
| `\lstinputlisting`, `\inputminted`, `\verbatiminput`, `\import{dir}{file}`, `\InputIfFileExists`, files loaded by a local `.sty` | LaTeX | Not an include command |
| Fonts looked up by family name (`\setmainfont{Foo}`, Typst `font: "Foo"`) | LaTeX, Typst | The file name never appears |
| `read("notes.txt")`, `image("figs/" + name + ".png")` | Typst | Computed or unlisted paths |
| `\includegraphics{f1}` for `f1.pdf` | LaTeX | Stems of two characters or fewer are not matched |

A missed file shows up as a compile error that looks like the person's own mistake. That is worse than slowness, and it is why section 7 drops only what is provably unrelated and falls back on failure.

### 3.8 The desktop app deletes what a manifest omits

`sync_hosted_workspace` (`crates/librepaper-companion/src/local/render/workspace.rs:282-330`) writes changed files, then deletes any earlier file missing from the manifest (lines 299 and 322). A scoped manifest would therefore remove files from the local workspace. Local renders also run the person's code, which can read anything. Section 10 keeps local workspaces whole.

### 3.9 What engines already report

- **LaTeX:** the engine's reply carries `inputFiles`, from the recorder. It surfaces as `pass.inputs` (`latex/driver.js:381`), and today it is used only to find nested `.aux` files (`worker.js:640-662`). It is the read set for LaTeX, available now.
- **Typst:** `needs` reports only what was not found, meaning packages and fonts (`needs.js`). It does not report successful reads.
- **Markdown, Quarto, Org, HTML:** report nothing.
- **Desktop app:** tracks no per-file reads.

## 4. Vocabulary

- **Asset:** a binary file in the project, addressed by SHA-256 (`tree.digests`).
- **Asset store:** the one client module that fetches, verifies and caches asset bytes (section 5).
- **Document root:** a text that renders alone (`rendersAlone`, `opening.js:227-243`): a LaTeX file with `\documentclass`, a Typst, Org, Quarto or Markdown file that nothing includes (and, for Quarto and Markdown, whose name does not start with `_`), or any HTML file.
- **Render scope:** the subset of the tree handed to one preview render.
- **Read set:** the project paths a render actually opened, as its engine reports them.
- **Escalation:** rerunning a scoped render with the whole tree.

## 5. The client asset store

`web/src/lib/figures.js` becomes the asset store. Every caller that needs asset bytes goes through it: previews, exports, downloads, workspace previews and `figureUrlFor`.

### 5.1 Bounded fetching

- **At most 6 requests in flight per tab** (`ASSET_CONCURRENCY = 6`), in a FIFO queue. Six matches a browser's HTTP/1.1 per-origin limit, so nothing is lost even without HTTP/2, and it is well below the deployment's 64 permits.
- **Priority:** the render that is waiting goes ahead of prefetch. A request carries `priority: "render" | "prefetch"`, and render requests jump the queue.
- **Shared queue:** one queue serves all callers, so a preview and an export started together do not double the load.

### 5.2 Retries

- **429 with `Retry-After`:** wait that long, capped at 30 s. Then retry, at most 3 times. `transfer_concurrency` refusals use the shorter server value from 6.2.
- **Network error or 5xx:** retry after 0.5, 1 and 2 s, with jitter.
- **403, 404 or a digest mismatch:** no retry. The asset is `missing`, as today.
- **After retries run out:** the asset is `missing`, and the render's existing behaviour applies (`preview-render.js:260-263`).

### 5.3 The persistent cache

- **Storage:** bytes live in the Cache API under a cache named `librepaper-assets-v1:<partition>`, keyed by the request URL `/assets/<sha256>`. They are verified on write and trusted on read: the key is the digest, and the bytes were checked against it when stored.
- **Partition:** the signed-in account id, or `anonymous` plus the visitor cookie id. One person's cache never serves another account in the same browser profile.
- **Sharing across documents:** within a partition, an asset with the same digest in two documents is fetched once. The person already holds those bytes, so serving them for a second document leaks nothing.
- **Clearing:** sign-out and "forget local data" (`localDataForgotten`) delete the partition's cache.
- **Size:** a cap of 512 MiB per partition, evicted least recently used. Storage pressure (`navigator.storage.estimate`) halves the cap.
- **When storage is unavailable:** in a private window or with a refused quota, the store falls back to today's in-memory map.

The in-memory map stays as the first level, so a render reuses bytes it already holds without a cache read.

### 5.4 Readers below Editor

The server still checks `references_asset` for readers below Editor. The cache does not bypass that check: a commenter's partition only ever holds bytes the server has already served to that commenter.

## 6. Serving assets

### 6.1 Cache headers

`blob_response` for asset reads sets:

```
cache-control: private, max-age=31536000, immutable
etag: "<sha256>"
```

`private` keeps shared caches out, because access is per document. `immutable` holds because the digest is in the URL. `default_cache_control` already leaves a response alone when it has both an `ETag` and its own `Cache-Control`. The client drops `cache: "no-store"` and relies on the asset store's cache. The browser's HTTP cache is a second level that costs nothing to keep.

The comment at `figures.rs:144` becomes true.

### 6.2 A per-principal share of transfers

The deployment-wide semaphore stays. In front of it, each principal (account or visitor key) gets at most `artifact_transfers_per_principal` concurrent transfers, 8 by default.

- **Over the share:** the response is 429 with `reason: "transfer_concurrency"` and `retry-after: 1`. That is a share that frees up in milliseconds, not a budget that resets once a minute.
- **The 60-second retry stays** for `request_budget` and `work_concurrency`.
- **Configuration:** `[cost] artifact_transfers_per_principal` in `librepaper.toml`, documented next to `artifact_transfers`.
- **Metrics:** a new gauge, `librepaper_http_artifact_transfers_refused_total{reason}`, next to `librepaper_http_artifact_transfers_active` (`metrics.rs:734`).

With 5.1 in place a well-behaved client never reaches the share. The share stops one runaway tab, or an older client, from starving everyone else.

## 7. The render scope

### 7.1 What is dropped

A preview render receives the tree minus:

1. **Unrelated texts:** document roots other than `main`, plus texts that only they reach through `includesOf`, followed recursively. A text that `main` also reaches is kept. A text that no root reaches is kept too: it may be a style, data or template file read some other way.
2. **Unreferenced figures:** assets whose extension is an image or a figure format (`png jpg jpeg gif webp svg pdf eps ps tif tiff bmp`) and that no kept text names. A figure is named when its full path, its base name, or its stem appears as a whole word in a kept text. A stem of any length counts, unlike PR 94's minimum of three characters. Every other asset is kept: fonts, archives, data, anything unknown.

Everything else stays. Non-document texts such as CSS, SCSS, Lua, Python, R, `.txt`, `.tex` partials without `\documentclass`, YAML and templates are small, and compilers read them in ways an include walk cannot see.

This is the inverse of PR 94's allow-list. It removes what is clearly unrelated and keeps whatever might matter. On the PR's project the 704 texts are mostly notes, which are document roots, and the 84 assets are figures, so the speed-up survives.

### 7.2 Format exceptions

- **Authored HTML:** a `main` of HTML keeps every HTML text, because frames are not includes.
- **Quarto projects:** when `_quarto.yml` exists, every text under `_extensions/`, and every file `_quarto.yml` names, is kept. Its paths are read as plain strings; it is not run.
- **Local execution:** a render that runs on this computer (`localExecution`) is never scoped. Section 10 applies instead.
- **Read sets:** when a read set exists for `(main, format)` (section 9), the figure rule also keeps every asset in the read set, whether named or not.

### 7.3 Escalation

A scoped render that fails with an error a missing file could explain is rerun once with the whole tree. Errors that count:

| Renderer | Errors that count |
|---|---|
| LaTeX | ``! LaTeX Error: File `…' not found``, `! I can't find file`, a recorder entry for a project path outside the scope |
| Typst | `file not found`, `failed to load file`, an unknown font or image |
| Pandoc (Markdown, Quarto) | `Could not fetch resource`, `could not find`, `withBinaryFile: does not exist` |
| Org, HTML | Any failure. They are cheap to rerun |

- **Success on rerun:** if the full-tree rerun succeeds, its page is shown. `(main, format)` is then marked `full` until the project's list of paths changes, meaning a file is added, removed or renamed. While marked, every render of that page uses the whole tree.
- **Failure on rerun:** if the full-tree rerun also fails, its error is the one shown. The person sees the same error they would see today.
- **Background:** escalation happens without the person seeing it. The preview's busy state covers both attempts.

Escalation bounds the cost of a wrong scope at one extra render, and makes it impossible for scoping to show an error that would not happen without it.

### 7.4 Where scope is computed

`renderScope(tree, { format, main, readSet, full })` lives in `web/src/lib/reader/render-scope.js`, as PR 94 put it.

- **Callers:** it is called by `preview-render.js` only. `liveTreeNow()` stays whole for exports and downloads.
- **Cost:** the result is memoised on the session tree's cache key plus `main`, so a keystroke in the open file recomputes only the open file's references.
- **Purity:** it has no side effects. Section 12 lists its tests.

## 8. Render identity

The identity used by the `finished` map and the LaTeX acceptance check becomes `snapshotDigest(scope)` instead of `snapshotDigest(tree)`.

- **Unrelated edits:** editing a file outside the scope leaves the identity, and the cached page, unchanged.
- **Scope changes:** a change that alters the scope itself changes the identity, as it should. Adding an include and adding a file both count.
- **Escalated renders:** a render escalated to the whole tree uses the whole tree's identity, so it is never confused with the scoped one.

`snapshotDigest` itself does not change. Its input does. The digest still has to match the shared projection fixtures (`SPEC-unification.md`), and it does, because the function is unchanged.

## 9. Observed read sets

Structure guesses a scope. Engines can report one.

### 9.1 Recording

- **What:** after a successful render, the read set is the list of project paths the engine opened, minus generated files (`.aux`, `.bbl`, `.toc` and similar).
- **Where it is kept:** in memory, keyed by `(slug, main, format)`, and persisted next to the stand-in page in `preview-cache.js`, with the same eviction.
- **LaTeX:** `pass.inputs` from the recorder (`latex/driver.js:381`) is already available. `latex.js` passes it up, and the preview records it.
- **Typst:** the engine needs to report successful reads. That is a requirement on `wasm-typst`, which lives outside this repository: export an `accessed()` list beside `needs()`. Until it exists, Typst uses sections 7.1 to 7.3 alone.
- **Pandoc, Org, HTML:** no reporting is planned. Structure and escalation are enough for them.

### 9.2 Use

A read set changes only the asset rule (7.2): every asset it lists is kept, and prefetched at `prefetch` priority when the page is selected. It never removes anything section 7.1 keeps. A read set cannot make a render fail, because it only ever adds files.

A read set is a hint, recomputed after every successful render. A stale one costs a prefetch at most.

## 10. Desktop app renders

Local renders keep the whole tree, because they run the person's code and because `sync_hosted_workspace` deletes files the manifest leaves out (3.8). What changes is how the bytes get there.

### 10.1 Manifest first

A new route on the local service, `POST librepaper/local/jobs/plan`, takes the manifest `{path, sha256, size}[]` without bytes. It answers with the digests the app's content store lacks:

```json
{ "missing": ["<sha256>", "..."] }
```

`POST librepaper/local/jobs` then carries the manifest plus only the missing files. The app copies or hard-links the rest from its store into the workspace.

### 10.2 The app's store

- **Location:** the companion keeps files by digest under `~/.local/state/librepaper/local/store/<aa>/<sha256>` (the platform equivalent elsewhere).
- **Size:** capped at 2 GiB, evicted least recently used. Files the current workspaces use are pinned.
- **Verification:** every upload is verified against its digest before it is stored, as job uploads are today.

### 10.3 Wire version

- **Version:** this is an addition, so the local wire becomes 3.3.0 (`SPEC-wire.md` 7.1).
- **Older apps:** a page that sees an app older than 3.3.0 falls back to today's full upload.
- **Fixtures:** the new route gets wire fixtures checked by Rust and by Node, like the other render routes (`SPEC-wire.md`).
- **Limits:** `MAX_UPLOAD_BYTES` and `MAX_FILES` (`protocol.rs:17-27`) apply to the manifest and the upload as today.

## 11. Lazy materialisation

Engines that can fetch on demand should not be handed bytes up front.

- **Typst, Markdown, Quarto, Org:** `handOver` already calls `set_asset_url` (`renderer-wasm.js:150-167`). For these engines the preview hands over URLs for assets not yet in memory: `blob:` URLs backed by the asset store, or the asset URL itself, which the store intercepts. Bytes are added only for assets already in hand. An asset the engine never opens is then never fetched.
- **LaTeX (both engines):** the Emscripten file systems need bytes when a file is opened. `FS.createLazyFile` with a synchronous fetch from the worker can serve them, but it blocks the engine on the network mid-compile. This phase is deferred until sections 7 to 9 have been measured. The read set already keeps LaTeX close to what it needs.
- **Authored HTML:** keeps data: URLs. The output goes to a sandboxed frame on another origin, which cannot reach the asset store.

## 12. Testing

- **`render-scope.mjs` (unit):**
  - every case in 3.7 keeps its file: Quarto CSS, Lua and extensions, LaTeX listings and fonts, Typst `read()`, two-character stems;
  - other document roots and their includes are dropped;
  - shared includes are kept;
  - HTML keeps its frames;
  - a read set adds assets;
  - `full` returns the tree unchanged;
  - the input is not mutated.
- **`figures.mjs` (unit):**
  - at most 6 in flight;
  - render priority goes first;
  - a 429 with `Retry-After` is honoured, and three failures give `missing`;
  - a digest mismatch is not retried;
  - one partition does not serve another;
  - clearing on sign-out.
- **`preview-render` (unit):**
  - an error from the escalation table triggers exactly one full-tree rerun;
  - a successful rerun marks the page `full`;
  - adding a file clears the mark;
  - an edit outside the scope does not change the identity.
- **Server:**
  - asset reads carry `private, max-age=31536000, immutable` and the `ETag`;
  - the per-principal share answers 429 with `retry-after: 1`;
  - another principal is unaffected.
- **Companion:**
  - `jobs/plan` reports only missing digests;
  - a job with a partial upload builds the same workspace as a full one;
  - an app older than 3.3.0 is served the full upload.
- **Browser:** a fixture project with 200 notes and 100 figures.
  - Opening one page fetches only that page's figures.
  - A second open in a new tab fetches none.
  - No request is refused.
- **Fixture:** PR 94's project shape is the regression benchmark. Record figure requests on open, time to first page, and time for a file switch, before and after.

## 13. Phases

Each phase ships alone and is useful alone.

| Phase | Content | Risk to correctness |
|---|---|---|
| 1 | Asset store (5), cache headers and per-principal share (6) | None. Same inputs, fetched better |
| 2 | Render scope with escalation (7) and scoped identity (8) | Bounded by escalation: at worst one extra render |
| 3 | LaTeX read sets (9). Typst once `wasm-typst` reports reads | None. Read sets only add files |
| 4 | Content-addressed local uploads (10), wire 3.3.0 | None. Workspaces stay whole |
| 5 | Lazy materialisation for Typst, Markdown, Quarto, Org (11) | Low. An engine fetch can fail where a gathered file would have failed the same way |

Phase 1 alone fixes the 429s and the refetch on every new tab, and it is where to start. PR 94's author could be offered phase 2. Their measurements are the benchmark, and their `render-scope.js` file is the right place for it.

A batch endpoint stays out. Measure after phase 1: if a cold open of a figure-heavy project is still slow over HTTP/2, a `POST /api/documents/{slug}/assets` returning several digests can be specified then.

## 14. Open questions

1. **Cache cap:** is 512 MiB per partition right for the browser cache? Large LaTeX projects with many PDF figures may want more; phones less.
2. **Revoked access:** should a revoked share purge the cached bytes? Today a revoked reader keeps whatever their browser already rendered. The cache extends that to assets they fetched. The proposal is no purge, because the digest-keyed bytes are ones they already saw.
3. **Cross-document sharing:** cross-document sharing within a partition makes "do I already have this?" observable to page script in the same account. The page is ours, so this is accepted, but it is noted.
4. **Typst read reporting:** the `accessed()` export needs a release of `wasm-typst`. Its schedule is outside this repo.
5. **Quarto escalation:** Quarto previews render in the browser through Pandoc. Do any Quarto errors from a missing CSS or Lua file not match the escalation patterns (7.3)? Each pattern needs a fixture before phase 2 ships.
