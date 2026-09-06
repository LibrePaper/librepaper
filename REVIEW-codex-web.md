# Deep code review of `web/`

Reviewed 2026-09-05, against HEAD `5a29aef86d1dcf005d0c18342ce1b1a844455481` and the working-tree web sources.

## Fix follow-up — 2026-09-06

The original findings and source locations below describe the pre-fix tree. All 26 findings have now been addressed in the implementation. Three GPT-5.6 Luna agents implemented the browser changes; the primary reviewer reviewed their diffs, corrected integration regressions, and added the corresponding server changes.

| Findings | Implemented correction |
| --- | --- |
| 1 | Server state-vector replies avoid full-state uploads on ordinary joins; large offline updates use bounded, validated multipart messages and retain one durability acknowledgment. |
| 2 | PDF names hash the exact captured tree, including stable text IDs and asset sizes; compile ownership prevents coalesced or stale results from being mislabeled. |
| 3 | Complete comment/reply submissions remain available through failures, snapshots, and reloads; explicit retries use author-scoped durable IDs to avoid duplicates. |
| 4, 5, 9 | Failed compiles preserve the preview, remembered distributions initialize their workers, and compiler replacement/error paths settle pending work. |
| 6, 11, 20 | Secondary-file edits invalidate previews through the debounce path; file switching preserves editor state and reconciles inactive remote edits; diagnostic navigation updates the parent selection. |
| 7, 8, 16, 17, 18 | Frame readiness belongs to one navigation, buffered previews replay after readiness, stale requests are discarded, and history/main-format transitions choose the correct frame. |
| 10, 15, 21, 24 | PDF workers are destroyed; same-text repaints restore annotations; historical extraction restores assets or reads stored PDFs; figure anchors use stable asset digests. |
| 12, 13, 14, 19, 22, 23, 25, 26 | Correct asset renaming, complete-or-failed ZIP exports, UTF-8 ZIP flags, in-memory link keys, upload error recovery, the LaTeX check launch path, safe entity decoding, and reconnect cancellation. |

Follow-up validation includes the web checks/build, the Rust suite and Clippy, a real-browser editor regression, and the assembled application's browser smoke test with offline comment recovery. A direct cross-language test compares the browser digest against Rust's canonical tree bytes, including Unicode, numeric, and prototype-like paths. Real Chromium checks found zero live PDF workers after each of four renders and confirmed that region annotations survive same-text repaint and image reordering.

The full LaTeX engine/corpus checks still skip because the local mirror and corpus PDFs are absent; worker lifecycle tests and a small generated PDF cover the relevant control paths. Historical PDFs that were never stored remain explicitly unavailable. Legacy image anchors whose old blob-URL digest cannot identify an image are left unmatched instead of being attached to a different image by position.

## Original review summary

The web application provides collaborative editing, document rendering, annotations, file management, and revision history. Its separation between the reader shell and document frame is useful, but several transitions between these subsystems fail: large-document joins, LaTeX initialization and rendering, file switching, annotation repainting, and historical navigation.

**Verdict: Request changes.** The highest-priority findings affect synchronization, lost comment drafts, and the integrity of stored PDF revisions. This report contains 26 findings grouped by severity. P1 means a high-impact defect to fix promptly; P2 means a substantive correctness or resource-management defect; P3 means a narrower defect. Finding numbers are stable references, not an implementation sequence. Start with the small LaTeX corrections in findings 4 and 5; the recommended batches below account for implementation effort as well as impact.

## Scope and verification

- Reviewed first-party JavaScript, Svelte components, styles, HTML entry pages, Vite configuration, checks, and browser/interop tools under `web/`. Dependencies, generated bundles, WASM binaries, fonts, and other upstream assets were excluded as review targets.
- Read relevant server/session handlers only to establish the actual contracts the web code calls. Findings below identify web-side problems; this is not another review of `crates/`.
- `npm run check` exited 0. Vocabulary, diagnostics, synchronization, timeline, passage-search, and LaTeX-log checks ran. The LaTeX engine check **skipped** because `latex/mirror` is absent; the PDF viewer check **skipped** because the corpus PDFs are absent.
- `npm run build` exited 0 for both the shell and agent. It reported one Svelte warning about the initial capture of `panel`; that capture is explained by the code and is not treated as a defect.
- Ran temporary Node reproductions against the actual collaboration, LaTeX, room, ZIP, and synchronization modules. Used extracted, unmodified Reader functions with controlled dependencies for rendering races and result-shape checks.
- Built the actual `Editor.svelte` into a temporary browser harness and ran it with the actual document agent in headless Chromium. Separately drove the freshly built PDF viewer with a small generated PDF and counted live workers over four successive previews.
- Reproduction harnesses were written under `/tmp/komodoc-review-*.mjs`; they are diagnostic artifacts, not repository tests. No production source was changed. Other working-tree changes were present or appeared during this review and were left alone.
- Findings marked **reproduced** have executable evidence described below. **Source trace** means the triggering sequence follows directly from the implementation, without claiming an end-to-end browser reproduction.

## High-priority issues

### 1. [P1] Every editor sends the entire document back through a smaller WebSocket limit

**Location:** `web/src/lib/collab.js:176–184`, `358–390`; supporting contract: `crates/komodoc/src/server.rs:1029`.

`start()` invokes `catchUp()` after every join, even when the browser has no unsent edits. `catchUp()` base64-encodes the complete Yjs document into one JSON message. The server limits incoming messages to 1 MiB. The HTTP reference mechanism only solves large state transfers in the opposite direction.

**Reproduced:** A main text of 800,000 ASCII characters, plus tiny metadata and a chapter, produced a **1,067,051-byte** catch-up message against a **1,048,576-byte** server limit. This size is below the ordinary document-size ceiling. Opening such a document as an editor therefore closes the socket, rejoins, and sends the same oversized state again. Unsaved edits cannot become durable through this loop. IndexedDB restoration and large pasted/imported text can also emit oversized individual updates at line 163.

**Fix:** Implement a bounded large-update path, and use the server's state vector to send only missing state when possible. Handle oversized initial updates as well as reconnect catch-up; merely increasing the limit slightly moves the failure threshold.

**Regression:** Open a near-limit document with no local edits, restore one from IndexedDB, and reconnect after an offline edit. Assert bounded transport, a stable socket, and a durability acknowledgment.

### 2. [P1] A PDF can be permanently stored under the hash of different source

**Location:** `web/src/components/Reader.svelte:891–899`, `957–1001`; `web/src/lib/latex.js:240–259`.

The render captures `treeNow()` first, then independently asks `/renderings/latest` for the server's current `live` hash. Asset fetching and network latency separate those operations. A local update may still be in transit, or another editor may change the document before the hash request is handled. Nothing proves that the returned hash describes the captured tree.

**Reproduced:** The actual `paintPreview()` function, given source snapshot A and a server response naming B, compiled A and called `holdRendering('B', bytesOfA, ...)`. The server accepts an existing checkpoint hash and cannot verify which source produced PDF bytes. Because renderings are immutable, this can poison B's stored rendering for subsequent readers.

This requires a snapshot/hash mismatch during the asynchronous interval; it does not happen on every compile. The controlled reproduction establishes the failure mechanism, not its frequency in a deployed application. Slow asset fetching makes the interval easier to encounter.

There is another mismatch opportunity in queue coalescing: replaced callers receive the newer tree's result, while retaining their own independently obtained `renderingName`. Suppressing out-of-order painting does not establish the identity of the compiled source.

**Fix:** Bind each compile result to a digest of the exact input tree using the same canonical digest definition as the server, or have the server supply an immutable snapshot and its digest together. Carry that identity through queue replacement and storage. Do not label local bytes with an unrelated query for current server state.

**Regression:** Delay asset fetches and synchronization, change source in another peer, and replace a queued compile. Verify that every stored PDF is associated only with the exact snapshot compiled.

### 3. [P1] Comment and reply drafts disappear after a failed offline submission

**Location:** `web/src/lib/room.js:52–64`; `web/src/components/Reader.svelte:283–297`, `324–331`, `339–343`; `web/src/components/CommentCard.svelte:91–97`.

When the socket is down, `send()` makes one REST request. A network failure only calls `connected(false)`; it neither preserves the outgoing message nor reports a submission failure carrying its temporary ID. Meanwhile the dialog clears the draft and the reply form clears its text. On reconnect, `hello` replaces the entire comment list with the server snapshot, discarding the optimistic comment/reply that never reached it.

**Source trace:** Go offline, submit a paragraph-long comment or reply, then reconnect. The only remaining copy was the optimistic row, and that row is replaced. There is no retry or draft recovery. A socket disappearing just after `send()` has a related acknowledgment gap.

**Fix:** As a first step, retain the draft until acknowledgment and surface failed attempts with a retry action; a persistent automatic outbox is not required to stop silently losing the text. If resending automatically, establish server-side deduplication for the submission ID. Reconcile `hello` with pending submissions rather than overwriting them.

**Regression:** Reject the fallback fetch, reconnect, and verify the full draft remains recoverable; also simulate a lost acknowledgment after a successful write.

### 4. [P1] A failed LaTeX compile erases the last successful PDF

**Location:** `web/src/components/Reader.svelte:990–1022`; `web/src/lib/renderers.js:190–193`; `web/src/agent/agent.js:511–520`.

The LaTeX renderer returns `{ pdf: null, diagnostics }` on a normal compilation failure and has no `html` property. After the false `if (pdf)` branch, `html !== null` is nevertheless true for `undefined`. The Reader sends an HTML preview with `html: undefined`. The agent turns that into an empty string and replaces the viewer's body, removing the last good pages. The viewer also retains its `stage` reference to the now-detached element, so subsequent successful PDF renders can be drawn off-document.

**Reproduced:** Passing a failed LaTeX render through the actual `paintPreview()` function emitted exactly this undefined-HTML preview. The agent and viewer branches establish the subsequent DOM behavior.

**Fix:** Give the renderer result an explicit discriminant or initialize the absent output channel to `null`. Treat success as an actual page payload, for example `typeof html === 'string'`; route no-page results to diagnostics without touching the frame. Keep the viewer's stage lifecycle consistent if its DOM is ever replaced.

**Regression:** Render a valid PDF, introduce a TeX error, then fix it. The valid page must remain visible during the error and update afterward.

### 5. [P1] Remembered LaTeX selections do not initialize a compiler after reload

**Location:** `web/src/components/Reader.svelte:1525`; `web/src/lib/latex.js:61–65`, `130–138`, `240–241`; `web/src/components/LatexCard.svelte:58–66`.

`prepare()` sets `latexReady` from `latex.chosen()`, which only reads localStorage. On a fresh page, the module's `name`, `worker`, and `loaded` are still null. The only production caller of `latex.choose()` is the card's explicit Choose handler, but `latexReady` suppresses that card. Consequently a previously configured editor attempts to compile with no initialized distribution and receives “no LaTeX distribution has been chosen.”

**Reproduced:** With a remembered `swiftlatex-pdftex` choice, `chosen()` returned that selection and `compile()` immediately rejected because no `choose()` had run. The Reader path sets readiness using precisely that storage check.

**Fix:** Restore the remembered distribution with `await latex.choose(saved)` during initialization and mark it ready only after successful loading. On failure or a removed distribution, return to a usable selection card. Distinguish a remembered preference from a running compiler.

**Regression:** Choose once, reload, and open a second LaTeX document. Both should compile without requiring the user to rediscover the toolbar's distribution picker.

## Major issues

### 6. [P2] Changes inside secondary files do not invalidate other peers' previews

**Location:** `web/src/lib/collab.js:219–222`, `317–321`; `web/src/components/Reader.svelte:1276–1284`.

`watchSource()` follows only the main `Y.Text`. `onFiles()` uses shallow map observers, which notice adding/removing/replacing entries but not editing the nested `Y.Text` inside an existing secondary file. An editor's CodeMirror callback happens to trigger rendering for the file currently open in that browser; readers and editors looking at another file have no corresponding callback.

**Reproduced:** Editing an existing `chapter.typ` produced zero source-watch callbacks and zero directory-watch callbacks while the session tree did contain the new text. A paper importing that chapter remains visually stale for another reader until a separate event triggers rendering.

**Fix:** Observe nested text changes across the complete files map, e.g. through `files.observeDeep(...)`, and debounce preview invalidation independently of which source file is open.

**Regression:** Two browsers open the same multi-file document; one edits a secondary chapter while the other has only the preview or the main file open. Both rendered documents must update.

### 7. [P2] Frame navigation and early PDF delivery leave previews blank

**Location:** `web/src/components/Reader.svelte:171–177`, `847–885`, `1585`, `1818`; `web/src/components/Preview.svelte:19–22`.

`frameReady` becomes true once and is never reset when `frameSrc` changes. A freshly navigated frame's `ready` therefore does not request its initial content. This matters when changing visibility reloads an empty document shell: highlights are sent to it, but its document is not repainted.

Stored PDFs also have an initial-load race. `paintRendering()` can finish before the iframe listener is ready; `tell()` silently sends/drops the message, yet `renderedSha` and `everPaintedShown` advance. The first `ready` calls `paintPreview()` again, but the equal-SHA check prevents resending the PDF the frame never received.

**Source trace:** Delay iframe navigation while allowing API/PDF requests to finish, or change visibility on an already-rendered document without editing again.

**Fix:** Track readiness and displayed content per frame navigation. Reset the frame generation and replay its latest content after the new frame announces readiness. Advance “displayed SHA” on delivery/acknowledgment, not merely on fetching bytes.

**Regression:** Exercise slow iframe load, visibility changes, and frame reload after a stored PDF has already been fetched.

### 8. [P2] Overlapping stored-PDF requests display one revision while naming another

**Location:** `web/src/components/Reader.svelte:847–885`.

Unlike the normal render path, `paintRendering()` has no generation guard. It writes shared `rendering` before awaiting PDF bytes and reads that same shared variable afterward to set `renderedSha`.

**Reproduced:** Start a rendering fetch for A, switch to B, resolve B, then resolve A. The final frame receives **A's bytes**, while `viewing`, `rendering.sha`, and `renderedSha` all say **B**. Future B requests are then suppressed by the equal-SHA guard. The version banner can therefore substantively misidentify the document on screen.

**Fix:** Capture the requested SHA and a generation locally. After each await, discard a superseded request; use the captured SHA when committing the displayed result. Apply the same guard to failures and transitions back to live content.

**Regression:** Resolve A/B requests in both orders and assert the displayed bytes, banner, and cached displayed SHA always agree.

### 9. [P2] Switching distributions during a compile permanently stalls the compile queue

**Location:** `web/src/lib/latex.js:154–156`, `188`, `264–298`.

`choose()` terminates the old worker without rejecting its active compile promise. `running` stays true because only that promise's `finally` resets it. New calls queue forever even after the replacement worker announces readiness. Runtime worker errors have a similar problem: `onerror` still rejects the already-resolved initialization promise rather than the active compile promise.

**Reproduced:** Start a compile on distribution A, choose B before A responds, and request another compile. A is terminated; both compile promises remain pending; B receives zero compile messages.

**Fix:** Give the worker an owned active-job record. Termination, runtime error, and cancellation must settle that job and reset the queue. Use a worker generation so late callbacks cannot mutate a newer worker's state.

**Regression:** Switch distributions during a compile and inject a worker error after initialization. A subsequent valid compile must finish in both cases.

### 10. [P2] Each PDF preview leaks a live pdf.js worker

**Location:** `web/src/lib/pdf/render.js:49–60`, `71–121`.

Every call creates a new `pdfjs.getDocument()` loading task. Neither the task nor its resolved document is destroyed on completion, failure, or generation cancellation. `page.cleanup()` releases page resources; it does not end the document's worker lifetime.

**Reproduced in Chromium:** Four successive previews of a one-page PDF left **1, 2, 3, then 4 live worker targets**, while exactly one rendered page remained in the DOM. A long editing session accumulates workers and their document state on every compile.

**Fix:** Retain and destroy the loading task/document in a `finally` block once canvas/text rendering has finished, or explicitly own a single active document and dispose of the previous one when replacing it. Cover every early return and failure path.

**Regression:** Repaint repeatedly and cancel overlapping paints; worker/resource counts should remain bounded.

### 11. [P2] Switching files destroys the editor state it is supposed to preserve

**Location:** `web/src/components/Editor.svelte:272–297`.

The editor-construction effect reads the reactive `file` prop. Changing files therefore invokes its cleanup, destroys the view, and clears every cached state before recreating the editor. The second effect and `states` map do not prevent this. Undo history and caret position are lost on an ordinary file switch.

**Reproduced in Chromium with the actual component:** After a local edit, switching A → B → A changed undo depth from **1 to 0**, reset the caret to **0**, and produced a different `EditorView` instance.

**Fix:** Separate view construction/destruction from reactive file selection. Do not track `file` in the construction effect; use the selection effect to swap state. When making cached states survive, also synchronize their text from Yjs before reuse so remote edits made while a file is inactive cannot leave stale CodeMirror text.

**Regression:** Switch between files after editing and scrolling; verify selection, undo, and the latest remote text survive.

### 12. [P2] The asset Rename control writes into the text-file path map

**Location:** `web/src/components/Files.svelte:108–120`, `207–214`; `web/src/components/Reader.svelte:1390–1392`; `web/src/lib/collab.js:288–289`.

The UI offers Rename for assets as well as text files and forwards only the ID and new path. Assets use their path as ID, but `session.renameFile()` always writes `paths.set(id, path)`. An asset is actually stored in the separate `assets` map. Its name therefore does not change, and an unrelated path-map entry is created.

**Reproduced:** Put `figure.png`, rename its ID to `renamed.png`, and inspect `session.tree()`: the original digest remains under `figure.png`; `renamed.png` is absent.

**Fix:** Dispatch renames by file kind. For an asset, move its digest between asset-map keys in one Yjs transaction; update the selected figure and open-file identity as well. Keep the destination kind compatible with the original file.

**Regression:** Rename an asset, then render, download, reconnect, and inspect it from another peer.

### 13. [P2] Project downloads silently omit figures that could not be fetched

**Location:** `web/src/lib/figures.js:61–85`; `web/src/components/Reader.svelte:1434–1450`.

`gather()` deliberately catches each failed asset fetch and returns only successful files. That may be a defensible best-effort preview policy, but `downloadTree()` reuses it to build an apparently complete project ZIP without checking whether every digest was returned.

**Source trace:** Let one referenced asset return 403, 404, or a network error, then download the project. The ZIP contains the referencing source but omits the figure, with no error shown. A user relying on the download as their project copy receives incomplete data.

**Fix:** Add a strict gathering mode for export, or compare requested and returned paths and refuse the download with a recoverable list of missing files. Do not silently describe a partial archive as the whole project.

**Severity rationale:** Whether to permit an explicitly partial export is a product choice. Silently omitting requested project files is the correctness defect. The smallest fix is the requested/returned-path check and an error, not a new export subsystem.

**Regression:** Fail one of several asset fetches and assert that no apparently successful complete-project download is offered.

### 14. [P2] ZIP filenames are UTF-8 bytes without the UTF-8 flag

**Location:** `web/src/lib/zip.js:89`, `107`.

Entry names are encoded with `TextEncoder`, but the general-purpose flags are zero in both the local and central headers. There is no Unicode filename extra field either. ZIP readers that use the legacy encoding when the UTF-8 bit is absent interpret accented/non-Latin names incorrectly, which can break source imports after extraction.

**Reproduced:** An archive containing `é.tex` has UTF-8 filename bytes with general-purpose bit 11 unset in its headers.

**Fix:** Write `out.u16(1 << 11)` in both flag fields. Keep the actual filename bytes UTF-8.

**Regression:** Round-trip accented and non-Latin paths through an independent ZIP reader, including a source file referencing another such path.

### 15. [P2] Repainting unchanged text removes region annotations without restoring them

**Location:** `web/src/agent/agent.js:511–535`, `605–611`.

Every HTML preview replaces the body and therefore destroys region overlays. Only text highlights are restored locally. The Reader restores regions when the agent publishes `ready`, but `publish()` returns immediately if the joined text is unchanged. Image changes, styling changes, or simply rendering the same document again can consequently remove every region box indefinitely.

**Reproduced in Chromium:** Paint one region, send a preview with identical text and image markup, and inspect the frame. Region-box count changes **1 → 0**, while ready-message count remains **1 → 1**. There is no notification asking the shell to repaint the overlays.

**Fix:** Track document/DOM generations separately from text equality. Restore remembered region overlays locally or publish a repaint-ready signal even when the text is unchanged. Image-list changes must also update the shell's figure metadata.

**Regression:** Annotate a figure and re-render with unchanged prose, changed image/style only, and identical HTML. Keep the annotation visible in all applicable cases.

### 16. [P2] Historical navigation applies stale checkpoint fetches after newer choices

**Location:** `web/src/components/Reader.svelte:546–561`, `1184–1189`.

`showCheckpoint()` assigns the fetched point directly to `viewing` without a selection generation. Clicking A then B can finish with A selected if its response arrives last. Leaving the history panel or clicking Back to now also does not invalidate an outstanding checkpoint fetch; that fetch can put the application back into history after the user explicitly left it.

**Source trace:** Delay A's checkpoint response, choose B or leave the history panel, then resolve A. The response still assigns `viewing` and repaints.

**Fix:** Increment a navigation generation when choosing a checkpoint, returning to live content, or leaving history. Commit fetched state only if its generation is still current; optionally abort superseded requests.

**Regression:** Control response order for rapid A/B selection and for leaving history during a pending request.

### 17. [P2] Back to now can leave an old HTML checkpoint on screen

**Location:** `web/src/components/Reader.svelte:557–561`, `732–734`, `799–812`, `944–954`.

Public/link HTML documents normally run as a live framed page. Viewing a checkpoint replaces that page with historical HTML. On Back to now, `paintsTheFrame` becomes false, so the code only calls `refreshFramedPage()`. If the live source has not changed since it was originally framed, it equals `framedSource` and that function returns. The historical DOM stays visible even though the history banner has disappeared; the original page's scripts are not rerun either.

**Source trace:** Open an HTML document as a reader, view a different historical version, then return to now without any intervening live edit.

**Fix:** Record that the frame currently contains a checkpoint and force a live-page reload when leaving that state. Source-string equality is insufficient to determine what the frame is displaying.

**Regression:** Check both visible content and document script execution after a reader exits an HTML checkpoint.

### 18. [P2] Changing the main file's format does not change the loaded frame

**Location:** `web/src/components/Reader.svelte:741`, `1334–1339`, `1585`.

`refreshFiles()` updates `sourceFormat` when the main file is renamed or changed, and `framePath` is derived from it. But `frameSrc` is an independently assigned string; changing `framePath` does not navigate the existing iframe. Moving from Markdown/Typst to LaTeX therefore sends PDF messages to the raw HTML shell. That shell's agent explicitly ignores PDF messages and has no viewer to handle them. Historical trees can also have a different main format from the live tree, while frame selection still follows the live format.

**Source trace:** Start with a `.md` main file, add a `.tex` file and make it main without reloading the reader.

**Fix:** Derive the required frame kind from the tree actually being displayed, and navigate/reinitialize the frame when that kind changes. Combine this with the per-navigation readiness handling in finding 7.

**Regression:** Switch main files between HTML/Markdown and LaTeX, and visit a checkpoint from before such a format change.

### 19. [P2] Large-state fetching forgets the in-memory link key when storage is unavailable

**Location:** `web/src/lib/collab.js:375–378`; `web/src/lib/storage.js:46–63`; `web/src/components/Reader.svelte:59`, `1267–1275`.

The Reader captures the incoming link key in `KEY`, so normal HTTP requests and the WebSocket can work even when localStorage writes fail. `collab.start()` instead rereads `keyFor(slug)` when fetching a referenced large state. With unavailable storage, that value is empty. A private document accessible through the supplied key can pass the initial requests but fail its state fetch. The fragment has already been removed, and link-copy helpers also depend on the failed storage write.

**Source trace:** Make localStorage throw, open a keyed private document large enough to use `y-state.ref`, and inspect the state-fetch headers: the in-memory key is not passed into `join()` and cannot reach that request.

**Fix:** Pass the active key explicitly into the session and reuse it for every request. Keep an in-memory key fallback for link generation when persistent storage is unavailable.

**Regression:** Open, synchronize, and copy a keyed private document with localStorage denied, for both inline and referenced state.

### 20. [P2] Diagnostic navigation changes the editor's file without updating the Reader's file

**Location:** `web/src/components/Editor.svelte:129–130`, `147–154`, `250–260`; `web/src/components/Reader.svelte:1110–1111`, `1883–1884`.

The diagnostic button calls `Editor.nextDiagnostic()`, which can `show()` a different file internally. The parent still holds the old `openFile`: there is no bound prop or callback updating it. The file list marks the wrong file, caret synchronization interprets the text using the old file's format, and clicking the parent-selected file again may do nothing because the prop has not changed.

**Source trace:** Keep the main file open, trigger a diagnostic in a secondary file, and use the diagnostic badge to navigate there. The component's `showing` changes; the Reader's `openFile` does not.

**Fix:** Make file selection a single owned state. Have diagnostic navigation request the parent to switch files and then move the caret, or bind the current file and publish every internal switch.

**Regression:** Navigate to a diagnostic in another file, assert the list selection matches the editor, then click the original file and exercise caret synchronization.

### 21. [P2] Historical passage extraction cannot handle asset-dependent Typst or LaTeX

**Location:** `web/src/lib/passages.js:39–46`; `web/src/components/Reader.svelte:587–611`.

`textAt()` discards the checkpoint's asset map and invokes the renderer with `digests: {}`. A Typst document requiring an image can fail to compile even though the original checkpoint was valid. Its null page is then treated as an empty document. For LaTeX, the function asks for `{ html }` from a renderer that returns PDF bytes; readers without a chosen compiler reject earlier, and a successful compile still yields no extracted text.

**Source trace:** Trace a removed passage in a valid Typst checkpoint containing `#image(...)`, or in a LaTeX paper. The historical text needed by `wentAt()` is not produced. Existing passage tests substitute an artificial text provider and do not exercise this path.

**Fix:** Reconstruct the full checkpoint tree and gather its assets. For LaTeX, extract normalized text from the stored PDF or another stored text representation, using the same rules as the viewer, without requiring readers to install a compiler. Treat unavailable renderings as unknown rather than empty text.

**Severity rationale:** The existing lookup fails for supported documents; that makes this a bounded feature defect, rather than a request for a new policy. A smaller interim fix can explicitly return an unsupported/unavailable result for these cases, preserving the distinction between unknown and absent, before implementing full extraction.

**Regression:** Trace an orphaned passage through actual rendered Typst checkpoints with assets and through stored LaTeX PDF revisions.

### 22. [P2] A network failure leaves the upload form permanently busy

**Location:** `web/src/components/Landing.svelte:247–269`.

`submit()` sets `busy = true`, awaits `upload(form)`, and only afterward resets the flag. A rejected fetch skips that reset and all error reporting. The Create project button remains disabled. Canceling and choosing another file does not reset `busy`, so the user cannot retry without reloading.

**Source trace:** Reject the multipart upload fetch rather than returning an HTTP error response.

**Fix:** Put upload and response handling in `try/catch/finally`, preserve the selected file on failure, show the network error, and clear `busy` in `finally`.

**Regression:** Reject the upload once and then succeed; the second attempt must be possible without reloading or losing the entered title.

### 23. [P2] The LaTeX browser check launches a nonexistent server script

**Location:** `web/checks/latex.mjs:122`.

The check spawns `latex/serve.mjs`, but the repository's server is `latex/tools/serve.mjs`. The missing-mirror guard currently skips before reaching this code, hiding the failure. Once the mirror is installed, the intended integration check cannot start its server and cannot validate the actual engines.

**Source trace:** The spawn path and repository file inventory disagree; `latex/tools/serve.mjs` is the existing implementation.

**Fix:** Spawn `join(REPO, 'latex', 'tools', 'serve.mjs')` and fail promptly if the child exits before readiness. Test the actual non-skipping entry path in CI or an explicit fixture-enabled job.

**Regression:** Run the check with a mirror and assert server readiness before opening the browser; missing script/startup failures should surface directly rather than as later timeouts.

### 24. [P2] Region identity is derived from ephemeral blob URLs instead of image content

**Location:** `web/src/agent/agent.js:324–334`, `367–372`; `web/src/lib/figures.js:67–69`.

`digestOf(image)` hashes the image URL string. Markdown figures receive a newly generated `blob:` URL in each page session, so the same stored image has a different annotation digest after reopening the document. The fallback is the image's numerical position. If another figure has been inserted before it, a saved region can then be painted on the wrong image instead of following the original asset.

**Source trace:** Annotate the second asset-backed Markdown image, close the page, insert another image earlier in the document, and reopen. The blob-string digest cannot match the previous visit, and `found[item.index]` now identifies a different figure. The WeakMap also does not invalidate a cached digest when an existing image element changes `src`.

**Fix:** Carry the stable asset digest into the rendered image metadata and use it for region identity, or hash actual immutable image bytes where necessary. Make positional fallback explicit when identity is unavailable, rather than silently attaching a known image's comment elsewhere.

**Severity rationale:** The reorder/reload trigger is narrower than the ordinary reload failures above, and its frequency was not measured. P2 reflects the consequence—silently attaching a review comment to a different figure. Schedule it after the common-path failures; this rating does not imply that it occurs frequently.

**Regression:** Reload after inserting/reordering images and after changing an existing image's source. Regions should follow stable content identity.

## Narrower issues

### 25. [P3] Out-of-range HTML numeric entities throw during caret synchronization

**Location:** `web/src/lib/sync.js:26–35`.

`decodeInPlace()` checks whether a parsed entity is finite, then calls `String.fromCodePoint()` without validating the Unicode range. HTML source may contain an invalid numeric reference; the browser renders a replacement character, but caret synchronization throws instead of declining to match.

**Reproduced:** `documentPlaceFor('<p>&#x110000; Some sufficiently long phrase to match</p>', 0, 'Some sufficiently long phrase to match', 'html')` throws `RangeError`.

**Fix:** Validate the range before conversion and normalize invalid references consistently with the intended HTML decoding rules. A malformed entity must not escape a heuristic matching function as an exception.

**Regression:** Include out-of-range, zero, surrogate, and malformed numeric references.

### 26. [P3] Closing a room does not cancel a queued reconnect

**Location:** `web/src/lib/room.js:27–43`, `66–69`.

The reconnect timer is not retained, and `connect()` does not check `closed`. Closing a room during its backoff interval therefore allows the queued timer to create another WebSocket after cleanup. The delayed disconnected callback is also left active.

**Reproduced:** Trigger `onclose`, call the returned `close()`, then run the queued 500 ms reconnect callback. A second WebSocket is constructed despite the closed flag.

**Fix:** Retain and cancel reconnect/disconnection timers in `close()`, and guard `connect()` with `if (closed) return`. Prevent callbacks from a retired socket from operating on a replacement socket.

**Regression:** Close during backoff and assert no more sockets or connection-state callbacks occur.

## Test coverage assessment

The pure-function checks are useful, especially diagnostic timing, text synchronization, log parsing, and timeline folding. They do not validate the most failure-prone ownership and lifecycle boundaries described above. The temporary reproductions found defects while the repository's normal check command remained green.

Highest-value additions are:

1. A real Reader initialization/reload test with a remembered compiler and delayed iframe readiness.
2. Cross-peer editing of secondary files, large state joins, and failed/retried annotation submissions.
3. Controlled response ordering for checkpoint selection, stored PDFs, and rendering identity.
4. Component lifecycle tests preserving editor history and aligning diagnostic file selection with the parent.
5. Repeated PDF rendering with worker cleanup assertions, plus asset-only/unchanged-text annotation repaints.
6. Strict export tests with failed assets and international filenames.
7. Fixture-enabled compiler/viewer jobs that actually execute the currently skipped integration paths.

The broader `web/tools/browser-smoke.mjs` was read but not executed against the existing binary: that binary embeds a shell snapshot and would not establish which current web sources it tests without rebuilding the Rust application. The temporary component/agent harnesses and freshly built viewer directly exercised the reviewed web code instead. This review did not perform an upstream dependency/CVE audit or assert that external TeX distributions work.

## Positive feedback

- `Preview.svelte` checks both message origin and the specific iframe window before forwarding messages into the shell.
- Shell API writes consistently use the same-origin marker header, and normal keyed requests carry the document key in a header. Initial link-key transport via a fragment avoids sending it with ordinary HTTP navigation.
- Collaboration distinguishes server acknowledgments from socket connectivity and retains unacknowledged Yjs state, which provides a sound basis for repairing transport limits and failure recovery.
- Binary asset buffers are copied rather than transferred into compilation workers, avoiding accidental detachment of the session's asset cache.
- The diagnostic timer is isolated behind an injectable clock, making its timing semantics directly testable.
- PDF rendering uses an off-document staging tree and a generation counter, avoiding incremental publication of half-rendered pages. Those patterns should be extended to request ownership and cleanup.

## Questions and decisions for follow-up

These are design decisions for the fixes, not prerequisites for recognizing the defects:

- Should export abort when any file is unavailable, or offer a clearly labeled partial archive? Current silent omission is unsafe for a project-download action.
- Should a disconnected annotation remain a recoverable draft or be queued for automatic retry? Either policy needs acknowledgment and reconciliation.
- What should historical passage lookup say when a checkpoint lacks a usable rendering? Distinguish unknown from absence of the passage.

**Recommended implementation batches:**

1. Fix the ordinary LaTeX failures in **4 and 5** first. Follow with the small, localized corrections in **14, 22, 23, 25, 26, and 12**. The result-shape fix in 4 is small, but verify recovery to a subsequent good PDF as well as preservation of the previous one.
2. Address **1** as a focused transport change, covering reconnects, initial state restoration, and large individual updates. Its importance is not reduced by doing the quick corrections first.
3. Address **2** as a snapshot-identity change and **3** as a submission-recovery change. They both involve acknowledgment/identity, but they need separate invariants and regression tests; they need not be coupled into one implementation. Retaining failed comment drafts can proceed independently of the digest design.
4. Repair frame/request lifecycle and resource ownership (**7–10, 16–18**), then the remaining file, annotation, history, and export defects. Use the smallest correction that establishes each stated invariant; the suggested architectures are options, not requirements to redesign whole subsystems.

Add regression tests alongside these fixes. Before treating the assembled application as validated, rebuild the Rust binary with the updated web assets and run `web/tools/browser-smoke.mjs`; the isolated reproductions in this report do not replace that integration check. Findings 4 and 5 also need fixture-enabled LaTeX coverage beyond the smoke test's current scope.
