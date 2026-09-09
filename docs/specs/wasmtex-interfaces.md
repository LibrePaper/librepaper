# WasmTex implementation: module boundaries and interfaces

Status: implementation contract for [wasmtex.md](wasmtex.md).
Every module below is owned by one work package. The shapes here are the
agreement between packages; a package may add fields but must not rename or
remove what is listed, and must not edit another package's files.

Naming: a **release** is one immutable LibrePaper browser distribution (engines,
formats, package snapshot, VM image identity). A **snapshot** is one immutable
document tree plus compile settings. A **job** is one attempt to compile a
snapshot on one backend. A **backend** is `browser`, `local` or `vm`.

## 1. Mirror layout and manifest (package A)

`latex/tools/wasmtex.mjs` builds the WasmTex half of `latex/mirror`. The
existing `manifest.json` gains two top-level keys and keeps `version: 1`.

```
latex/mirror/
  manifest.json
  wasmtex/<engineRelease>/                 engine files from a staged wasm-latex release
    wasmtex-pdftex.worker.js wasmtex-pdftex.js wasmtex-pdftex.wasm
    wasmtex-pdftex.fmt wasmtex-pdftex-resolver-evidence.js ...
    wasmtex-bibtex.* wasmtex-bibtex8.* wasmtex-makeindex.*
    wasmtex-xetex.* wasmtex-xetex.fmt.gz wasmtex-dvipdfm.*
    wasmtex-luatex.* wasmtex-luatex.fmt.gz
    BUILD-RECEIPT.*.json LICENSE-MANIFEST.json
    NOTICES/LICENSE NOTICES/THIRD_PARTY_NOTICES.md NOTICES/licensing.md
  texlive/<snapshot>/<2hex>/<16hex>-<name>   package files, digest-named
  texlive/<snapshot>/bloom-filter.v2.bin      generated over the index below
  biber-vm/<vmRelease>/...                    package F, see section 6
```

`<engineRelease>` is `librepaper-<sha256 of the staged MANIFEST.json>` for a
release imported from wasm-latex (the first mirrored release used the
upstream `releaseId`, `2026-8b7946970153c52e`); `<snapshot>` is the
upstream package snapshot id (`2026-ba38749b8714505a`). Directories are
immutable; a new id is a new directory.

Manifest additions:

```json
{
  "version": 1,
  "default_release": "2026-8b7946970153c52e+2026-ba38749b8714505a",
  "releases": {
    "2026-8b7946970153c52e+2026-ba38749b8714505a": {
      "id": "2026-8b7946970153c52e+2026-ba38749b8714505a",
      "digest": "<sha256 hex of the canonical JSON of this entry without `digest`>",
      "engine_release": "2026-8b7946970153c52e",
      "snapshot": "2026-ba38749b8714505a",
      "texlive": "2026",
      "kernel": "LaTeX2e 2026-06-01",
      "base": "wasmtex/2026-8b7946970153c52e/",
      "texlive_base": "texlive/2026-ba38749b8714505a/",
      "engines": {
        "pdftex":  { "worker": "wasmtex-pdftex.worker.js", "format": "wasmtex-pdftex.fmt", "files": ["wasmtex-pdftex.worker.js", "wasmtex-pdftex.js", "wasmtex-pdftex.wasm", "wasmtex-pdftex-resolver-evidence.js", "wasmtex-pdftex.fmt"] },
        "xetex":   { "worker": "wasmtex-xetex.worker.js",  "format": "wasmtex-xetex.fmt.gz",  "files": [...] },
        "dvipdfm": { "worker": "wasmtex-dvipdfm.worker.js", "files": [...] },
        "luatex":  { "worker": "wasmtex-luatex.worker.js", "format": "wasmtex-luatex.fmt.gz", "files": [...] },
        "bibtex":  { "worker": "wasmtex-bibtex.worker.js", "files": [...] },
        "bibtex8": { "worker": "wasmtex-bibtex8.worker.js", "files": [...] },
        "makeindex": { "worker": "wasmtex-makeindex.worker.js", "files": [...] }
      },
      "files": { "<name>": { "url": "wasmtex/2026-8b7946970153c52e/<name>", "sha256": "...", "size": 123 } },
      "bibliography": {
        "bibtex": "0.99e",
        "biblatex": "3.21",
        "control_file": "3.11",
        "biber": { "compatible": ["2.21"], "incompatible_hint": "biblatex 3.21 needs Biber 2.21" }
      },
      "vm": null,
      "source": {
        "wrapper_revision": "44c5861fcdf729838205b00b96ac9509bc7fb677",
        "corresponding_source": { "url": "...", "sha256": "..." },
        "build_receipts": ["BUILD-RECEIPT.pdftex.json", "..."],
        "reproduced": false
      },
      "licences": { "wrapper": "MIT", "pdftex": "GPL-2.0-only", "xetex": "GPL-2.0-only AND LicenseRef-XeTeX", "luatex": "GPL-2.0-only", "bibtex": "LicenseRef-BibTeX-Web2C-Notices AND LGPL-2.1-or-later", "notices": "wasmtex/2026-8b7946970153c52e/NOTICES/" },
      "sizes": { "pdftex": 5807474, "xetex": 0, "luatex": 0, "texlive_initial": 6785176 }
    }
  },
  "texlive": {
    "2026-ba38749b8714505a": {
      "upstream": "https://texlive.corca.ai/snapshots/2026-ba38749b8714505a/2026/",
      "bloom": { "url": "texlive/2026-ba38749b8714505a/bloom-filter.v2.bin", "sha256": "...", "size": 0 },
      "files": { "pdftex/26/amsmath.sty": { "url": "texlive/2026-ba38749b8714505a/8c/8c1f...-amsmath.sty", "sha256": "...", "size": 0 } },
      "absent": { "pdftex/26/nothere.sty": true },
      "initial": ["pdftex/26/article.cls", "..."]
    }
  }
}
```

`bibliography.control_file`/`biblatex` are read from the snapshot's
`biblatex.sty` (`\blx@bcfversion`, `\ProvidesPackage` date/version). `vm` is
filled by package F. `initial` lists the keys every corpus document needed:
the compact initial resource set the browser prefetches in parallel.

`default_release` names the validated default; `releases` retains older ones.
`texlive.<snapshot>.files` keys are `<engine>/<kpathsea format code>/<name>`
exactly as the WasmTex workers build them (`pdftex/26/amsmath.sty`; the XeTeX
and LuaTeX workers also use the `pdftex/` prefix, see their controllers).

### Serving (package A, Rust and dev server)

The browser fetches everything from `<base>/latex/` on its own origin. Both
`crates/librepaper/src/server/latex.rs` and `latex/tools/serve.mjs` must answer:

| Request | Answer |
| --- | --- |
| `texlive/<snapshot>/<engine>/<format>/<name>` (5 parts, `<name>` not digest-shaped) | Look `<engine>/<format>/<name>` up in `manifest.texlive[<snapshot>].files`; serve the digested file with header `fileid: <16hex>` and `Cache-Control: no-cache`; **404** when absent (WasmTex reads any status >= 400 as absent; it is not SwiftLaTeX). |
| `texlive/<snapshot>/bloom-filter.v2.bin` and any digest-shaped path | Static, immutable. |
| `wasmtex/<engineRelease>/<file>` | Static, immutable. |
| `manifest.json` | `no-cache`. |
| `packages/...` (legacy SwiftLaTeX protocol) | Kept working until package G removes it. |

The dev server's `--record` mode fetches an unknown `texlive/...` name from
`manifest.texlive[<snapshot>].upstream + <engine>/<format>/<name>` once,
verifies it is a 200, stores it digest-named, and records absent names in
`absent`. Recording is never on in a check.

## 2. Browser modules (packages B1, B2, B3)

All under `web/src/lib/latex/` except the public `web/src/lib/latex.js`.
Every module is plain ESM that runs unbundled under Node for checks (no
Svelte, no Vite-only syntax). Tests live in `web/checks/latex-*.mjs` and are
added to `bun run check` by their owner.

### 2.1 Shared value shapes

```js
// A project tree, as the reader already produces it (unchanged).
Tree = { main: string, texts: {path: string}, assets: {path: Uint8Array}, digests?: {path: sha256}, files?: {...} }

// Project compile settings (persisted with the project, see section 4).
Settings = { engine: "auto"|"pdflatex"|"xelatex"|"lualatex", release: string|null }

// Engine names are exactly these strings everywhere: "pdflatex" | "xelatex" | "lualatex".

// The immutable identity every compile receives and every result echoes.
Job = {
  id: string,            // `${project}:${generation}`
  project: string,       // slug
  generation: number,    // monotonically increasing per page load
  snapshot: string,      // sha256 hex over canonical tree digest + settings + release (see B2)
  inputs: string,        // sha256 of source inputs only (tree-digest.js snapshotDigest)
  main: string,
  engine: "pdflatex"|"xelatex"|"lualatex",
  release: string,       // release id from the manifest
}

// One backend attempt's log.
Attempt = { stage: "browser"|"local-biber"|"vm-biber"|"native", backend: "browser"|"local"|"vm", ok: boolean, log: string, reason?: string, tool?: string }

// The result `latex.compile` resolves with. Never rejects for a document that
// does not compile; rejects only for a job that was canceled or superseded
// (error.name === "Superseded") or an internal failure.
Result = {
  ok: boolean,
  pdf: Uint8Array|null,
  synctex: Uint8Array|null,     // gzip bytes of the .synctex.gz, or null
  log: string,                  // the final TeX log (primary backend)
  attempts: Attempt[],          // every stage that ran, in order, both backends' logs preserved
  diagnostics: Diagnostic[],    // latex/log.js shape, paths project-relative
  seconds: number,
  job: Job,
  provenance: Provenance,
  failure: null | { kind: "tex"|"resources"|"init"|"timeout"|"bibliography"|"local-unavailable"|"tool-missing"|"incompatible"|"vm"|"native", message: string, stage: string }
}

Provenance = {
  backend: "browser"|"local",             // who produced the PDF
  bibliography: null|"bibtex"|"local-biber"|"vm-biber"|"native",
  engine: "pdflatex"|"xelatex"|"lualatex",
  release: string|null,                   // browser release id, null for native
  tools: { tex?: string, bibtex?: string, biber?: string, distribution?: string, vm?: string }
}

// Bibliography job contract, identical for local and VM backends.
BiberRequest = {
  job: Job,
  stem: string,                           // job name, e.g. "main"
  bcf: Uint8Array,                        // <stem>.bcf bytes
  files: {path: Uint8Array},              // every .bib and biber config (biber.conf, .bibrc) and any bcf-referenced file, project-relative
  identity: string,                       // bibliography input identity (B2, section 2.5)
}
BiberResult = {
  ok: boolean,
  bbl: Uint8Array|null,
  blg: string,
  exit: number,
  tool: { name: "biber", version: string, backend: "local"|"vm" },
  incompatible?: boolean,                 // control file version mismatch, from the blg
  error?: string
}
```

### 2.2 Public API: `web/src/lib/latex.js` (package B2)

```js
export const DEBOUNCE = 1500;
export const DEFAULT_BASE = "/latex/";
export function at(url)                                      // mirror base, as today
export function configure({ project, settings, mayCompile }) // called by the reader when a document opens; resets generation on project change
export function settings()                                   // current Settings
export function setSettings(next)                            // engine/release change; invalidates route and cached bibliography
export async function releases()                             // { default: id, current: id, available: [{id, texlive, kernel, sizes}] } from the manifest
export function resolveEngine(tree, settings)                // "pdflatex"|"xelatex"|"lualatex" (delegates to latex/engine.js)
export function compile(tree, { manual = false } = {})       // Promise<Result>; queue: one running, one queued, newest wins
export function cancel()                                     // discard running and queued
export function status()                                     // Status snapshot (below)
export function subscribe(listener)                          // listener(Status); returns unsubscribe
export function tryBrowser()                                 // reset a session-native route
export const local = {                                       // re-exported from latex/local.js
  status, connect(code), disconnect, retry, setAddress(url), address(), capabilities, rescan, openApp
};
export const resources = {                                   // re-exported from latex/resources.js
  size(), clear(), readiness(), persist()
};
```

```js
Status = {
  phase: "idle"|"loading"|"compiling"|"checking-local"|"local-needed"|"local-biber"|"vm-preparing"|"vm-biber"|"native"|"ready"|"failed",
  message: string,        // the exact wording from SPEC "Failure presentation"
  backend: "browser"|"local"|"vm"|null,
  progress: { done: number, total: number, scope: string }|null,   // scope names what is measured, e.g. "engine and format"
  route: "browser"|"native",             // session route
  release: string|null,
  engine: string|null,
  local: LocalStatus,                    // section 2.6
  lastResult: Result|null
}
```

Status message wording, verbatim: `Loading browser compiler`, `Compiling in
browser`, `Checking local LibrePaper`, `Local connection needed`, `Running local
Biber`, `Preparing browser bibliography support`, `Updating bibliography in
browser`, `Compiling locally`, `Current preview ready`, `Compilation failed;
previous preview shown`, `Local LibrePaper is unavailable`.

### 2.3 Engine selection: `web/src/lib/latex/engine.js` (package B1)

```js
export const ENGINES = ["pdflatex", "xelatex", "lualatex"];
export function directiveOf(source)     // "% !TEX program = xelatex" / "%!TEX TS-program = lualatex" -> engine or null; only recognised names, never free text
export function detect(source)          // conservative: fontspec|unicode-math|polyglossia|xeCJK|xetexko -> xelatex; luacode|luatexja|\directlua|luaotfload -> lualatex; comments stripped; else null
export function resolveEngine(tree, settings) // settings.engine !== "auto" ? it : directive ?? detect ?? "pdflatex"
export function needsBiber(source)      // \usepackage[...]{biblatex} without backend=bibtex -> true (early hint only)
```

### 2.4 Engine adapter: `web/src/lib/latex/wasmtex.js` + `web/src/lib/latex/worker.js` (package B1)

`worker.js` is the module worker the controller talks to. It owns one release
at a time and lazily creates the nested WasmTex engine workers it needs
(pdftex | xetex+dvipdfm | luatex, bibtex, bibtex8, makeindex) from
`<base><release.base><worker>` using `<base><release.texlive_base>` as the
TeX Live endpoint. It restores the engines' execution state between passes
exactly as WasmTex's own drivers do (the controllers snapshot the heap).

Protocol (every request carries `id`; every reply echoes it):

```
in  { id, cmd: "configure", base, release: <manifest release entry>, texlive: <manifest texlive entry> }
out { id, ok: true, engines: [...] }
in  { id, cmd: "stage", engine, tree: Tree, generated: {path: Uint8Array} }   // fresh /work: flushcache, then write every file; `generated` are valid aux/bbl/ind from an earlier pass of the SAME snapshot
out { id, ok: true }
in  { id, cmd: "tex", engine, main }                                           // one pass; XeTeX includes the dvipdfm stage
out { id, ok: true, status: number, pdf: ArrayBuffer|null, synctex: ArrayBuffer|null, log: string, inputs: string[]|null, outputs: {path: ArrayBuffer} }  // outputs = every file in /work whose name matches <stem>.{aux,bcf,idx,ind,ilg,bbl,blg,toc,lof,lot,out,run.xml} plus any *.aux under directories the recorder saw
in  { id, cmd: "bibtex", stem, eight: boolean }                                // BibTeX or BibTeX8 on <stem>.aux; requires the aux and .bib/.bst files already staged in that engine
out { id, ok: true, status, bbl: ArrayBuffer|null, blg: string }
in  { id, cmd: "makeindex", stem }
out { id, ok: true, status, ind: ArrayBuffer|null, ilg: string }
in  { id, cmd: "write", path, bytes: ArrayBuffer }                              // e.g. a BBL returned by Biber
in  { id, cmd: "read", path }        out { id, ok, bytes: ArrayBuffer|null }
in  { id, cmd: "retire" }                                                        // terminate every nested worker (memory pressure, release change)
out { id, failed: "message" }                                                    // any command
out { cmd: "progress", done, total, scope }                                      // unsolicited, during configure/first tex
out { cmd: "downloading", file }                                                 // unsolicited, from the engines
```

Rules: a `tex` reply's `pdf` is null unless the engine exited 0 or 1 and wrote
a PDF in this pass; the worker deletes `<stem>.pdf`/`<stem>.xdv` before each
pass so a stale output can never turn a failed run into success. The format
is preloaded from the release (`loadformat`); the compact initial set
(`texlive.initial`) is prefetched through `resources.js` and injected with
`preloadtexlive` before the first pass. The bloom filter is loaded when the
manifest has one. `preamble snapshots` are disabled (`setpreamblesnapshot
false`). Raw bytes cross the boundary as ArrayBuffers, never strings.

`wasmtex.js` is the host-side driver class the worker uses for each nested
engine (message queue with ids, init, format preload, write/mkdir/read,
run). It is written by us against the worker controllers in
WasmTex's `wasm-build/*-worker.js` at the pinned source revision; nothing
from WasmTex's `lib/` is imported.

### 2.5 Resources: `web/src/lib/latex/resources.js` (package B1)

```js
export function namespace(release)                 // `librepaper-latex-${release.digest.slice(0,16)}`
export async function fetchVerified(release, url, { sha256, size, signal })   // Cache Storage under the release namespace; verifies sha256 on a miss; discards and refetches a corrupt hit; a network error is thrown, never cached
export async function prefetch(release, entries, onProgress)                  // parallel (6 at a time) fetchVerified over [{url, sha256, size}]; progress {done,total,scope}
export async function size()                       // bytes held across all librepaper-latex-* caches
export async function clear()                      // deletes every librepaper-latex-* cache and the VM caches; never touches project storage
export async function readiness(release, keys)     // { ready: boolean, missing: string[] } for the keys a project has used
export function persist()                          // navigator.storage.persist once, refusal tolerated
export function remember(release, key)             // record that a project used a texlive key (localStorage, bounded)
```

### 2.6 Local bridge client: `web/src/lib/latex/local.js` (package B3)

```js
export const DEFAULT_ADDRESS = "http://127.0.0.1:8763/";
export function address() / setAddress(url)         // localStorage `librepaper-local-address`
export function status()                            // LocalStatus
export function subscribe(listener)
export async function probe({ force = false })      // one bounded health probe (2 s); caches a negative result for the episode (60 s backoff, doubling to 10 min); returns LocalStatus
export async function connect(code)                 // POST /connect with pairing code -> stores token per (origin, project) in localStorage `librepaper-local-pairings`
export async function disconnect()
export async function retry()                       // clears the negative cache and probes
export async function capabilities({ rescan = false }) // Capabilities or throws {name:"Unauthorized"|"Unreachable"}
export async function runBiber(request: BiberRequest, { signal, onProgress }) // BiberResult; a rejected promise means the bridge could not be used (unreachable/unauthorized/refused), NOT a Biber failure
export async function runTex({ job, tree, engine, main }, { signal, onProgress }) // NativeResult below
export function openApp()                            // tries `librepaper://local/open`; returns instructions string for the CLI
export function configure({ project, origin })

LocalStatus = {
  state: "unknown"|"unreachable"|"denied"|"reachable"|"unauthorized"|"connected"|"incompatible",
  address: string, protocol: number|null, version: string|null,
  capabilities: Capabilities|null, checkedAt: number|null, error: string|null,
  instructions: string   // e.g. "Run `librepaper local start` and enter the pairing code it prints"
}
Capabilities = {
  tools: { pdflatex: Tool, xelatex: Tool, lualatex: Tool, bibtex: Tool, bibtex8: Tool, biber: Tool, makeindex: Tool },
  confinement: { available: boolean, kind: "bwrap"|"sandbox-exec"|"none", reason: string },
  platform: string, distribution: { name: string, year: string }|null
}
Tool = { available: boolean, version: string|null, note: string }
NativeResult = { ok, pdf, synctex, log, diagnostics: [], exit, provenance: {...Provenance, backend: "local"}, error?: string }
```

Wire protocol: section 5. All fetches use `mode: "cors"`, a 2 s health
timeout, `targetAddressSpace: "loopback"` where supported, and never send a
server cookie or the editor key. A permission-denied fetch error (Chrome
local network access) is state `denied` with instructions, not an exception.

### 2.7 Biber VM client: `web/src/lib/latex/vm.js` + `web/src/lib/latex/vm-worker.js` (package B3)

```js
export function supported()                          // { ok, reason } : WebAssembly, SharedArrayBuffer not required, navigator.deviceMemory >= 2 when known, not a known-unsupported UA
export async function prepare(release, onProgress)   // loads manifest.releases[id].vm descriptor, fetches runtime + boots the guest in a worker; progress scope "bibliography support"; idempotent; rejects with {name:"VmUnsupported"|"VmUnavailable"}
export async function runBiber(request: BiberRequest, { signal, onProgress }) // BiberResult with tool.backend "vm"; one job at a time; a newer request cancels a queued older one
export function retire()                             // stop the worker, drop guest memory; static resources stay cached
export function state()                              // "cold"|"loading"|"ready"|"busy"|"failed"
```

The worker boots v86 from the release's `vm` descriptor (section 6), stages
inputs under `/work/<job id>/` via the 9p filesystem, runs `biber
--output-format=bbl <stem>.bcf`, reads back `<stem>.bbl`/`<stem>.blg`,
deletes the job directory. Idle teardown after 5 minutes.

### 2.8 Controller and routing (package B2)

`web/src/lib/latex.js` (queue, jobs, status), `web/src/lib/latex/bibliography.js`
(detection from real aux/bcf/log, cache identity), `web/src/lib/latex/route.js`
(the routing table as a pure state machine over injected backends so it is
testable with doubles), `web/src/lib/latex/jobs.js` (Job identity, snapshot
digest = sha256 over `inputs + "\n" + engine + "\n" + release`).

Sequence per job (SPEC "Browser compilation controller"):

1. stage snapshot (+ valid generated state from the previous job of the same
   `inputs` when its bibliography identity is unchanged);
2. `tex`; on `status` failure or no PDF, go to fallback with `failure.kind`;
3. inspect outputs: `<stem>.bcf` present and log says `Please (re)run Biber`
   -> biber; `.aux` with `\bibdata`/`\citation` and log says `undefined
   citations` or no `.bbl` -> bibtex (bibtex8 when the aux/log says so);
   `.idx` -> makeindex; follow `\@input{...aux}` lines for nested aux files;
4. bibliography: cache key = sha256 over (bcf or aux bytes, every .bib bytes
   and path, every .bst/.cfg/.bbx/.cbx project files, biber config, engine,
   release id, tool identity, vm identity). Hit -> reuse BBL bytes; miss ->
   run BibTeX in browser, or Biber via `route.js` (local -> vm);
5. write BBL/IND, rerun `tex` until `rerun(log)` is false and the aux/toc
   files stop changing, at most 8 passes, whole job deadline 240 s (constant
   `DEADLINE_MS`), else `failure.kind = "timeout"`;
6. resolve with Result; a result whose `job.generation` is older than the
   newest resolved job is returned to its own caller but never becomes
   `status().lastResult`.

Routing (`route.js`): implements the SPEC table. Tracks per-snapshot attempts
`{ native: 0|1, vm: {identity: 0|1} }`; `route.js` exposes
`decide(event, state) -> { action, state }` where events are
`browser-failed`, `biber-needed`, `local-unreachable`, `local-tool-missing`,
`local-incompatible`, `local-biber-failed`, `vm-unavailable`, `vm-failed`,
`native-failed`, `native-ok`, `canceled` and actions are `try-local-biber`,
`try-vm`, `try-native`, `stop`, `show-browser`. Session-native route persists
in memory only (per page load, per project) and resets on `tryBrowser()`, a
settings change, or a new session.

A BiberResult with `incompatible: true` from the local route falls through to
native (if a suitable local TeX exists) and otherwise to the VM.

## 3. Reader integration (package C)

`web/src/components/Reader.svelte`, `Settings.svelte`, `LatexCard.svelte`
(deleted) and a new `LatexStatus.svelte` (compact status line under the
toolbar badge with the phase message, backend, and the actions `Connect local
LibrePaper`, `Retry connection`, `Try browser compilation`, `Open LibrePaper`).

- No chooser, no book icon. `latex.configure({project: SLUG, settings,
  mayCompile})` on open; `latex.subscribe` drives the badge and status line.
- `renderers.render` keeps its contract; the reader reads `provenance`,
  `attempts` and `failure` off the result and shows provenance in Diagnostics
  ("Compiled locally with pdfTeX 1.40.27 (TeX Live 2025); browser attempt
  failed: ...") without leaving an error state after a successful fallback.
- Settings panel for LaTeX: engine select (Automatic/pdfLaTeX/XeLaTeX/
  LuaLaTeX), pinned release with "Update to <default>" and "Revert", local
  connection status + controls + `librepaper local doctor` output, cache size +
  clear.
- Project settings persist in the Yjs `meta` map under keys `latex.engine`
  and `latex.release` (web/src/lib/collab.js gains `latexSettings()` /
  `setLatexSettings(next)`; readers never write them). A project with no
  release pin receives the default when an editor's browser first compiles it
  (write through `setLatexSettings`), never when a reader opens it.
- Rendering upload: `PUT /api/documents/<slug>/renderings/<sha>` unchanged,
  plus header `x-librepaper-provenance: <JSON Provenance, <= 2 KB>`; SyncTeX
  uploaded only from the same job as the PDF.
- Migration: delete the `librepaper-latex` localStorage key on load.

## 4. Server side (package R2)

- `crates/librepaper/src/document/history.rs`: `Tree` gains
  `#[serde(default, skip_serializing_if = "Option::is_none")] pub settings:
  Option<CompileSettings>` with `CompileSettings { engine: String, release:
  String }` (both `#[serde(default, skip_serializing_if = "String::is_empty")]`).
  A tree without settings serializes byte-for-byte as before, so every existing
  checkpoint keeps its sha. `room.rs` fills it from the Yjs `meta` keys
  `latex.engine`/`latex.release` when building the live tree.
  `web/src/lib/tree-digest.js` mirrors it (`settings` after `files` in the
  canonical JSON, only when present).
- Rendering provenance: `handle_rendering_upload` reads
  `x-librepaper-provenance` (JSON, <= 2048 bytes, must parse as an object),
  stores it beside the PDF (`room.put_rendering_provenance`), and
  `handle_rendering_latest` answers `provenance` when stored. Readers see it
  in the rendered note.
- `/api/config` gains `"latex_local": { "address": "http://127.0.0.1:8763/", "protocol": 1 }`.
- Tests: `crates/librepaper/src/tests/wasmtex_server.rs`.

## 5. Local bridge protocol v1 (packages R1a, R1b, B3)

Loopback only: bind `127.0.0.1` (and `[::1]` when available), default port
8763, `librepaper local start --port N` overrides. Every request must carry a
`Host` header of `127.0.0.1[:port]`, `localhost[:port]` or `[::1][:port]`,
otherwise 403 (DNS rebinding). CORS: preflight and responses echo the
request `Origin` only when that origin has a pairing or the route is
`health`/`connect`; `Access-Control-Allow-Private-Network: true` is answered
on preflight. No cookies. Bodies are bounded (JSON 64 KB, uploads 64 MB).

Base path `/librepaper/local/v1/`:

| Method, path | Auth | Body / answer |
| --- | --- | --- |
| `GET health` | none | `{ "service": "librepaper-local", "protocol": [1], "version": "<VERSION>", "instance": "<random 16 hex, per start>" }` |
| `POST connect` | none, rate limited 5/min | `{ "origin": "https://librepaper.example", "project": "<slug>", "code": "123456" }` -> `{ "token": "<32 bytes base64url>", "expires": <unix> }`. The code is printed by `librepaper local start` (and `status`), rotates every start, and is also accepted from the `LIBREPAPER_LOCAL_CODE` env for tests. Wrong code -> 403. |
| `POST disconnect` | bearer | revokes this token |
| `GET capabilities` | bearer | `Capabilities` (section 2.6). Paths never leave the app. |
| `POST capabilities/rescan` | bearer | same |
| `POST jobs` | bearer | multipart/form-data: part `job` (JSON `JobRequest`), then one part per input file whose field name is `file` and filename is the project-relative path. Server verifies each part against `manifest` (sha256, size), rejects absolute/`..`/backslash/control-character paths, symlinks, and unknown files. -> `202 { "id": "...", "status": "queued" }`; `409` when a newer generation of the same project is already queued; `413` over limits. |
| `GET jobs/<id>` | bearer (same project) | `JobStatus` |
| `GET jobs/<id>/files/<name>` | bearer | raw bytes of `pdf`, `synctex`, `bbl`, `blg`, `log`; 404 until done |
| `POST jobs/<id>/cancel` | bearer | terminates the process tree |
| `DELETE jobs/<id>` | bearer | removes workspace and outputs |

```json
JobRequest = {
  "protocol": 1,
  "kind": "biber" | "tex",
  "project": "<slug>", "origin": "https://...",
  "snapshot": "<sha256>", "generation": 12,
  "engine": "pdflatex" | "xelatex" | "lualatex",      // tex only; respected, never substituted
  "main": "paper/main.tex",                             // tex only
  "stem": "main",                                       // biber only
  "manifest": [ { "path": "main.tex", "sha256": "...", "size": 123 } ],
  "options": { "deadline_seconds": 300, "max_passes": 8 }
}
JobStatus = {
  "id": "...", "kind": "biber"|"tex", "status": "queued"|"running"|"done"|"failed"|"canceled",
  "stage": "staging"|"tex"|"bibtex"|"biber"|"makeindex"|"finished",
  "passes": 2, "exit": 0, "error": null|"...",
  "snapshot": "...", "generation": 12,
  "log_tail": "last 4 KB",
  "outputs": { "pdf": {"size": 1, "sha256": "..."}, "synctex": {...}, "bbl": {...}, "blg": {...}, "log": {...} },
  "diagnostics": [ { "severity": "error", "message": "...", "file": "chapters/01.tex", "line": 7 } ],   // paths normalised back to project-relative
  "provenance": { "backend": "local", "engine": "pdflatex", "tools": { "tex": "pdfTeX 3.141592653-2.6-1.40.27 (TeX Live 2025)", "biber": "2.21", "distribution": "TeX Live 2025" }, "confinement": "bwrap" | "none" },
  "incompatible": false     // biber: control file version mismatch detected in the blg
}
```

Execution rules (R1b): argument arrays only; app-selected executable paths;
`-no-shell-escape -interaction=nonstopmode -halt-on-error -recorder -synctex=1
-output-directory=<workspace>/out`; env is cleared except `PATH` (the
discovered tool's directory first), `HOME` (workspace), `TEXMFVAR`/
`TEXMFCONFIG` (workspace), `openout_any=p`, `openin_any=p`; `latexmk` is
never required. Deadline and output caps (PDF 64 MB, logs 4 MB). Cancellation
kills the process group. Confinement: `bwrap` on Linux when present
(`--ro-bind` the project and the TeX root, `--bind` the workspace,
`--unshare-net --unshare-pid --die-with-parent`), `sandbox-exec` on macOS,
none on Windows -> `confinement.kind = "none"` reported, never silent.

## 6. Biber VM release (package F)

`latex/tools/biber-vm/build.mjs` (Docker) builds a minimal 32-bit Debian
guest with Biber pinned to the release's `bibliography.biber.compatible`
version, exports a v86 9p filesystem (`fs.json` + `objects/`), and writes
`latex/mirror/biber-vm/<vmRelease>/` with:

```
vm.json          { "runtime": "v86", "version": "<libv86 revision>", "licence": "BSD-2-Clause", "memory_mb": 256,
                   "files": { "libv86.js": {url,sha256,size}, "v86.wasm": {...}, "seabios.bin": {...}, "vgabios.bin": {...}, "bzimage": {...}, "fs.json": {...} },
                   "objects": "biber-vm/<vmRelease>/objects/", "biber": "2.21", "perl": "...", "guest": "debian-bookworm-i386",
                   "boot": { "ready": "LIBREPAPER_VM_READY", "shell": "sh" }, "recipe": "latex/tools/biber-vm/", "sources": [...] }
libv86.js v86.wasm seabios.bin vgabios.bin bzimage fs.json objects/<sha>...
```

`<vmRelease>` is the sha256 (first 16 hex) over `vm.json` without its own
digest. The build writes `releases.<id>.vm = { "id": "<vmRelease>", "url":
"biber-vm/<vmRelease>/vm.json", "sha256": "...", "size": 0, "biber": "2.21" }`
into the manifest through `latex/tools/wasmtex.mjs --vm <dir>`. Guest boot
protocol is the one in
`latex/tools/biber-vm/worker.js`: serial console, `~% `
prompt, then a marker line. The guest has no network device.

## 7. CLI (package R1a)

```
librepaper local start [--port 8763] [--foreground]   # prints the pairing code and the address; runs until interrupted (--foreground is accepted but has no effect yet: start always stays attached)
librepaper local status                                # reachable? paired origins? code
librepaper local doctor                                # discovery report, confinement, suggestions
librepaper local disconnect [--origin URL] [--all]     # revoke pairings
librepaper local rescan                                # refresh discovery cache
```

State lives under `<config home>/librepaper/local/` (`service.json`: port,
instance, code, pid; `pairings.json`; `tools.json` discovery cache) and
workspaces under `<cache home>/librepaper/local/jobs/`. Never under the
project directory. `librepaper serve` never starts the local service.
