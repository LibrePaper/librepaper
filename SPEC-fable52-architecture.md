# Architecture review: where the codebase can get simpler and sturdier

Status: proposed 2026-11-09. Section 0 records what was done and decided the same day; where it disagrees with a later section, section 0 is right.

## 0. Outcome, 2026-11-09

**Proposal 1 (section 2) was done, in a different shape from its plan.** Eleven commits on `kata/c638-shell-menus` (Kata epic `xzhb`), not yet merged to `dev` when this was written.

- Owner modules in `web/src/lib/reader/`, each with a unit test: `projection-source`, `painted`, `preview-target`, `source-schedule`, `opening`, `arrangement`, `phone`.
- Components in `web/src/components/reader/`: `Menus`, `SaveTemplateDialog`, `DeleteCommentsDialog`, `ClearChatDialog`, `IdentityDialog`.
- `Reader.svelte` went from 5,507 to 4,682 lines. The painted-flags reset, which turned out to be written in six places, is written once.
- Piece 2 (a `Source` interface) was dropped: after the other pieces it would have wrapped three sites.
- Piece 6 did not use Svelte context. The panels are plain-prop components that browser tests mount alone, so the shell was split instead (menus, dialogs, arrangement, phone logic) and the panel snippets stay in Reader.
- Known weakness: `phone` takes about thirty injected inputs and `opening` about fifteen callbacks. They made the logic testable but moved the coupling behind a wide interface.
- The programme stops here. A line-count target was the wrong goal, and no failure mode motivates the remaining blocks (commands, comments, history).

**The other proposals, judged against the rule in `AGENTS.md`** (a concrete failure mode, its likelihood, the cost):

| Section | Proposal | Verdict |
|---|---|---|
| 3 | Document rules compiled to wasm | Not started, and the first verdict here was under-supported. It said no drift bug was known; nobody had looked. Kata `nef2` is one: the JavaScript `trackedSplice` records no change for an astral character where the Rust side does, and the shared fixture does not cover it. `05p4` and `jajc` show each rule change being written twice. The cost is still large (the browser's `LoroDoc` lives in the stock `loro-crdt` wasm instance). Worth a spike on the pure planners before deciding either way. |
| 4 | Accept and reject as server commands | Not built (Kata `qqnx`, closed). Grounded in a read of both sides: the race needs two editors to reject one change within the sync delay, the Rust `reject` lacks the browser's comment re-attachment and would need a port, and every decision would gain a round trip. |
| 5 | Generated wire types, `Refusal` cutover | Not built. The browser reads no refusal reasons (checked again when `storage_busy` was added: it acts on `retryable` and `vector` only), and the web app is not type-strict. The room protocol still has no shared fixture, which is the cheaper thing to add if wire drift ever bites. |
| 6 | Companion linked into the desktop shell | Not built (Kata `1qsv`, closed after a feasibility read). Almost nothing on the helper side could be deleted, because `librepaper local status` and three test files use the listener and its credential; the helper re-executes its own binary for agents and the macOS folder picker; it assumes it owns the process; and the dependency trees clash. The first verdict here, 'still a real simplification', was wrong. |
| 7 | One API client and one materializer | Not started, and not examined closely. The claim that a zip backup, a git folder and a render workspace differ too much to share one abstraction comes from a survey summary, not from reading the four writers. Sharing the asset download looks safe. |
| 8 | One source of truth for storage usage | Done in a narrower form (Kata `32q4`). The in-memory ledger could refuse a write the database would accept, so it was removed and the durable reservation decides. A shared quota-comparison helper and a single mark-deleting function were not built: the sites differ on purpose. |

**Found along the way.** Eight tests were failing on `dev`, and nothing ran them: three browser and e2e files (`nde9`, `3x27`, `ntgx`) and five deployment integration files (`7yy9`), each of which skips without a built binary or a database. Seven were stale. The eighth, `pending-budget-recovery`, was right: a stalled log flush held a deployment-wide lock and stopped typing in every document (`s18k`). The reservation's lock wait is now bounded, and `make check` runs the browser, e2e and integration suites. This paid off more than any proposal above.

This is a high-level review of the whole repository, written to find the few structural changes that would remove the most code and the most classes of bug. It is a list of proposals, not a plan. Each one needs its own spec before work starts.

It came from five read-only surveys on 2026-11-09 (Rust and JavaScript duplication, the storage engine, the server and room, the desktop app with the companion and sync, and the web app), plus spot checks by hand. Nothing was built or run. Line counts are approximate and will drift. Claims should be rechecked against the code before a piece starts.

## 1. Summary

The crate layering is clean: `base ← document ← engine ← room ← server`, with no cycles. The `Command` protocol in the sequencer and the rule that records are immutable and caches are recomputed are sound. `SPEC-unification.md` has mostly landed.

The remaining complexity has two sources:

- A few concepts exist twice: once in Rust and once in JavaScript, once in the shell and once in the helper, or once per client.
- One file, `web/src/components/Reader.svelte`, still owns the state that every feature of the web app shares.

| # | Proposal | What it removes | Size |
|---|---|---|---|
| 1 | State moved out of `Reader.svelte` into owner modules that panels read directly | Source-text tests, a reset written three times, the need to edit one file for every feature | Medium, in six pieces |
| 2 | Document rules written once, compiled to wasm for the browser | About 3,400 lines of mirrored JavaScript and the drift between the two sides | Large |
| 3 | Accept and reject as server commands | A documented concurrency limit and a browser-only write path | Medium |
| 4 | Generated wire contracts and a finished `Refusal` cutover | Hand-copied message types, 217 hand-written status replies | Medium |
| 5 | The companion linked into the desktop shell | About 1,500 lines that secure a hop between two binaries of one release | Medium |
| 6 | One API client and one materializer | Four folder writers, three asset downloaders, two git runners | Medium |
| 7 | One source of truth for storage usage | A ledger that drifts, a quota check written about eight times | Small to medium |

Section 9 lists one thing that looks like excess but should be left alone for now.

## 2. Finish moving state out of `Reader.svelte`

Revised 2026-11-09 after reading the file in detail. The first draft proposed one controller with explicit state machines and one cancellation rule. That diagnosis was partly wrong, and this section replaces it.

### What the detailed read found

`Reader.svelte` is 5,507 lines: a 4,572-line script and 935 lines of markup. It holds about 115 `$state` variables and 52 effects.

**The churn is feature wiring, not races.** Of 261 non-merge commits to the file in four weeks, about 57 read as fixes. Commits per region:

| Region | Lines | Commits |
|---|---|---|
| Markup | 4573–5507 | 142 |
| History, authors, tree, git, diagnostics | 1395–2049 | 92 |
| Imports | 1–157 | 83 |
| Phone shell, menus, downloads | 2888–3456 | 54 |
| Files and preview target | 3457–3954 | 47 |
| Panes | 2607–2887 | 42 |
| LaTeX, render, `sourceChanged` | 2050–2461 | 38 |
| Document and anchoring | 158–572 | 36 |
| Lifecycle (`prepare`, boot, teardown) | 3955–4369 | 34 |
| Room (`receive`, changes) | 1029–1394 | 13 |

Almost every feature edits the imports and the markup, because each panel is handed its data as props from Reader. `Collaboration` takes about 37 props and callbacks, `Files` about 30.

**Extraction is already under way, with a sound pattern.** `lib/reader/` holds 30 modules and 4,448 lines. Each is a factory that owns one concern: `workspace` (the directory), `annotations`, `timeline`, `history-source`, `collaboration` (room and session), `boot`, `preview-render`, `render-coordinator`. The newer ones own their state as a `$state` object that Reader reads. The render pipeline is already out, and its file header explains why.

**Race handling is mostly fine.** `render-coordinator.js` and `generation.js` are small and tested. Only two hand-rolled counters remain in Reader (`navigationGeneration`, `sourceGeneration`), and the renderer reads them on purpose through `facts()`. The other counters the survey found are private to single modules. A blanket move to `AbortSignal` would be churn for no gain.

**What is still wrong:**

- Reader still owns the state the older modules need. They read it back through closures (`facts: () => ({…})`, `getCanEdit`, `tree: () => treeNow()`), so about 30 variables are shared by everything.
- The "nothing is painted" reset is written by hand in three places: `clearPreview`, `updatePreviewTarget` and the renderer's `onEmpty`. Each resets the same six to eight flags (`everPainted`, `everPaintedShown`, `pageDrawn`, `documentEmpty`, `deliveredKind`, `activeSynctex`).
- The reader and editor fork is open-coded. `liveTreeNow` branches on session or projection, `projectEmpty` is derived two ways, and `refreshCurrentProject` with its five variables sits in the component.
- The tests that slice source text target exactly the functions not yet extracted: `prepare`, `filesChanged`, `updatePreviewTarget`, `clearPreview`, `treeNow`, `sourceChanged` (four times), `reportPersistence`, `beforeUnload`. Six unit files read `.svelte` source for logic.

### Verdict

Worth doing, in a narrower form: finish the extraction that has started, in the pattern the code already uses, then let panels read the owner modules directly. No big-bang controller, no new state-machine layer, no change to cancellation.

Reader stays the shell. Panes, the phone shell, menus and the keyboard are about 1,400 lines of view logic that belong in a component. Expect the script to drop by roughly a quarter from pieces 1 to 5. The larger gain is that features stop needing to edit it.

### Plan

Every piece is a move with no behaviour change. All of them edit `Reader.svelte`, so they run one at a time, each on a short-lived branch merged to `dev` before the next starts. Only piece 6 can be split across subagents.

1. **Projection source** (`reader/projection-source.svelte.js`). Owns `currentProject`, the etag, the in-flight fetch, the pending flag and timer, and `projectUnreadable`. Exposes `refresh()`, `schedule()` and `tree()`. Smallest and safest, so it goes first.
2. **A `Source` interface.** `tree()`, `empty`, `digest`, with two implementations: the projection source from piece 1 and a thin wrapper over the session. `liveTreeNow`, `treeNow` and `projectEmpty` stop branching.
3. **Preview target** (`reader/preview-target.svelte.js`). Owns `previewPath`, `sourceFormat`, the lock, `aloneSet`, the reason and choices, the navigation generation, and the painted flags behind one `reset()`. Takes `updatePreviewTarget`, `clearPreview`, `lockPreview` and `unlockPreview`. The three hand-written resets become one.
4. **Source schedule** (`reader/source-schedule.js`). Owns the source generation, the debounce timer and the frame handle. Takes `sourceChanged`, `filesChanged` and `scheduleTargetUpdate`.
5. **Opening** (`reader/opening.svelte.js`). Owns role (`mayEdit`), `connected`, `persistence`, `peers` and `settled`. Takes `prepare`, `startCollaboration`, `sessionEvents`, `reportPersistence`, the at-risk test and the teardown list. The eleven `doc-*` branches of `receive` that only forward to the session move into the session as one `receive(event)`, matching `annotations.receive` and `liveChat.receive`.
6. **Panels read the owners.** Reader puts one object in Svelte context that exposes the owner modules. Panels are converted one at a time to read from it, and their props are deleted from Reader's markup. One subagent per panel, starting with `Collaboration` and `Files`.

Each of pieces 1 to 5 deletes the source-slicing tests for the functions it moves and replaces them with tests that import the new module.

### Done looks like

- No unit test reads a `.svelte` file to test logic. `untranslated.mjs`, which lints markup text, is the exception.
- The painted-flags reset exists once.
- `mayEdit` in Reader's script is read from the opening module and appears in view decisions only.
- Adding a prop to a panel's data does not edit `Reader.svelte`.

### Checks per piece

- The unit files that covered the moved functions, with `node --test --experimental-transform-types`.
- `bun run check:types`, since `.svelte` and `.js` types change.
- The browser tests for the behaviour moved. No `make` target runs these: `editor-browser.mjs` for pieces 3 to 5, plus the reader and preview browser files that match the piece.

### Risks

- **Effect timing.** Several paths depend on `tick()` and on the order effects run. Moving a variable into a module's `$state` object keeps reactivity, but a derived value that becomes a getter can change when an effect re-runs. `reader-races.mjs` covers the known cases, so its tests are ported before the slices are deleted.
- **Conflicts with feature work.** The file takes about 65 commits a week. Small pieces merged quickly are the mitigation. A long-lived refactor branch would not survive.
- **Piece 6 widens what a panel can reach.** The context object should expose owner modules, never Reader's own variables, or it becomes a second global scope.

## 3. Document rules written once, compiled to wasm for the browser

### Evidence

| Concept | Rust (non-test lines) | JavaScript (lines) | Shared fixture |
|---|---|---|---|
| Tracked changes | `document/track.rs`, about 2,360 | `lib/track-changes.js`, 1,139 | Planner and unsuggest only |
| Comment anchors | `document/anchors.rs`, about 1,500 | `lib/anchors.js`, 1,494 | Planner only |
| Projection and digest | `document/projection/mod.rs`, about 550 | `lib/projection.js` and `projection-digest.js`, 338 | Yes |
| Opening | `document/opening.rs`, about 270 | `lib/opening.js`, 307 | Yes |
| Path admission | `base/paths.rs` and `document/paths.rs`, about 380 | `lib/paths.js`, 109 | None of its own |

- `anchors.rs` says it is a port of `anchors.js`.
- Both sides run in production on the same `changes` and `anchors` roots. The browser applies people's edits and the server applies agents' patches.
- The fixtures cover the pure planners. The code that writes to the Loro document (write, accept, reject, prune, resolve) has no shared test.
- Rust counts code points and JavaScript counts UTF-16 code units. The test runners convert.
- No crate in `crates/` is compiled to wasm today. The browser already loads seven wasm modules, so the delivery path exists.

### Proposal

- Split the pure parts of `librepaper-base` that the document crate uses (paths, lines, text, digest) into a small core crate with no tokio, axum or reqwest.
- Compile the planners to wasm first: `plan_edits`, the anchor planner, projection, opening and path admission. They are data in and data out and need no document handle.
- Delete the JavaScript planners in the same piece. The shared fixtures stay as the regression suite for the one implementation.
- The Loro writers stay as thin per-side code for now. The browser's `LoroDoc` lives inside the stock `loro-crdt` wasm instance, shared with CodeMirror, and a separately built module cannot hold a handle to it. Linking our functions into our own Loro build would solve that, but it means maintaining a Loro build, so it is out of scope until the planners have moved.

### Open questions

- The cost of crossing the wasm boundary on each keystroke for the anchor planner. The browser keeps an `AnchorIndex` so that an edit resolves no cursor, and that property must hold. Measure before committing.
- `track.rs` calls `SystemTime::now()` in `prune`. That needs a clock passed in.
- The Rust side uses `loro` 1.16.0 and the browser uses `loro-crdt` 1.16.1. Pin them together.

## 4. Accept and reject as server commands

### Evidence

- `docs/architecture/index.md` assigns review decisions to the server.
- Accept and reject run only in the browser (`lib/track-changes.js`), sent as ordinary Loro updates. There is no server or MCP route. Rust `track::accept` and `track::reject` are called only from tests.
- `docs/architecture/review.md` lists this limit: two editors who reject the same change at the same moment put the deleted text back twice.
- The docs already say decisions need a connection.

### Proposal

- Accept and reject become `Command`s through `Room::command`, like comments and restores. The sequencer gives them one order, so the second of two rejections finds the record gone and does nothing.
- The MCP tools gain a decision operation on the same command, if agents should be allowed to decide.
- The browser's accept and reject writers are deleted.

This changes behaviour, so it needs a decision before a spec. It also removes one of the writer pairs that section 3 leaves duplicated.

## 5. Generated wire contracts and a finished `Refusal` cutover

### Evidence

- The room protocol (`room/message.rs`, 15 message kinds) is hand-written again in `project-session.js`, `submissions.js` and others. It has no schema and no shared fixtures.
- The refusal codes in `room/refusal.rs` do not appear anywhere in `web/src`.
- `Refusal::status` is the one table from refusal to HTTP status. 217 hand-written `write_json(status, …)` calls bypass it (117 in `documents.rs`), and `access::deny` uses a different body shape.
- The only fixture-backed wire is the desktop one (`crates/librepaper-companion/wire/v3`).
- In the web app, only inbound frame messages have a single schema (`lib/frame-messages.js`). Worker, outbound frame and websocket messages are defined where they are used.
- The repository has no code generation for types.

### Proposal

- Derive TypeScript types from the serde types for the room protocol, the HTTP replies the browser reads, and the helper wire. The generated file is checked in and a test fails when it is stale.
- Handlers return `Result<T, Refusal>`. The hand-written status replies are deleted route by route.
- The browser branches on refusal codes, never on status numbers or message text.
- Each worker and frame protocol gets one module that defines its messages, imported by both ends.

## 6. The companion linked into the desktop shell

### Evidence

- The shell and the helper ship in the same release. The shell starts the helper as a separate process and talks to it over loopback HTTP.
- About 1,500 production lines exist to make that hop safe: `control-token.json`, the HMAC challenge on `health`, the `Host` and `Origin` gates, the `Forward` struct, the retry on a rejected credential and the watchdog that restarts the helper.
- Six things are defined on both sides because `desktop/` depends on no crate in `crates/`: the renderer request body, the agent declaration body, the health and endpoint types, the HMAC proof, the wire constants and the path splitting.
- The only stated reason for the split is a build one: the shell links the system webview, so it sits outside the workspace.

### Proposal

- `desktop/` depends on `librepaper-companion` by path and calls it in process. The `lp-local` scheme handler dispatches straight to the companion's router.
- The loopback listener, the control token, the challenge and the watchdog are deleted for the desktop path. The duplicated types are deleted.
- `librepaper local start` stays for development and tests if it is still wanted.

### What this does not remove

- Versioning between the page and the app (`SPEC-wire.md`). The page comes from the server and the app updates on its own schedule, so that skew is real.
- The bundled CLI. Agents spawn `librepaper mcp`, so the binary still ships.
- The login-shell `PATH` resolution, which a GUI app needs however the helper is hosted.

### Open question

- Whether a crash in the companion taking the window down is acceptable. Render jobs, git and agents are already subprocesses, so the exposure is the companion's own code.

## 7. One API client and one materializer

### Evidence

- Four separate implementations write a project into a folder, each with its own server fetch: `librepaper-sync` pull, `cli/export.rs`, `local/backup.rs` and `render/workspace.rs`.
- Three of them carry their own asset download.
- There are two git runners: `librepaper-sync/src/git.rs` (plain, synchronous) and `companion/local/git/` (bounded time and output, kills the process tree).
- The `ApiClient` from `SPEC-unification.md` was not built.
- The `lp` script (`skills/librepaper/scripts/lp`) re-implements the operation identity that `automation/mcp.rs` already handles.
- "Connection" names two unrelated things in the companion: a project linked to a Git repository, and a runner record for the MCP bridge. "Chat" names two in the server: project chat and the assistant channel.

### Proposal

- One `ApiClient` crate-level type, used by the CLI, sync, the companion and the MCP bridge.
- One `materialize(snapshot, sink)`, where the sink is a folder, a zip archive or a render workspace.
- One git runner, the bounded one.
- Rename one of each pair of overloaded words.

## 8. One source of truth for storage usage

### Evidence

- Usage is tracked in four places: the trigger-maintained `storage_usage` table, counter columns on `documents`, reservation rows, and the in-memory `StorageLedger`.
- A comment in `server/metrics.rs` says the ledger misses uploads, command rows, new documents and deletions until the next restart.
- The quota check is written again in about eight write paths (`insert_log_row_drawn`, `activate_log_base`, `insert_chat_messages`, `reserve_upload_admission` and others).
- "Mark a document as deleting" is written in three places.

### Proposal

- Postgres is the only record of usage. The in-memory ledger is deleted, or kept strictly as a read-through cache that is never adjusted by hand.
- One function charges usage inside a write transaction. Every write path calls it.
- One function marks a document as deleting.

## 9. Left alone on purpose

The engine carries a writer lease and epoch fencing (`storage/postgres/ownership.rs`, the `fenced` state in the sequencer), although the docs and the deployment describe a single process. It looks like excess, but the cost is modest, and the handling of ambiguous commits is needed anyway for dropped database connections. The useful step is to decide whether more than one process is ever a goal and to write the answer down. If it is not, the lease stays as a guard against starting two servers by mistake and the load-balancer reasoning in comments goes.

## 10. Order

1. Section 2 first. It needs no cross-language work, it can ship in small pieces, and it makes every later web change cheaper.
2. Section 5 next, since sections 3 and 4 both change messages between browser and server.
3. Sections 3 and 4 together.
4. Sections 6, 7 and 8 are independent of the rest and of each other.
