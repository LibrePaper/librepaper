# Deep review of `crates/`

Reviewed 2026-09-05. Primary snapshot: `729b624bf353f496e94746d093007ec10b33df9e`. Review author: Codex. Second review of priorities and an independent rerun of every probe on `5a29aef86d1dcf005d0c18342ce1b1a844455481`: Claude, 2026-09-05.

## Assessment

**This is a remediation work list, with cost bounding first.** Following a second review of priorities and threat models, this report identifies **34 defects: 15 P1 and 19 P2, plus one lease-policy decision (R24)**. The original 29 probes reproduce the reported behavior, but reproducing behavior does not by itself establish a security violation or its severity. Six items remain supported by direct code-path inspection. A follow-up probe additionally confirms R26’s effect on deliberate checkpoint deferral. The highest-impact failures concern unbounded storage, quota undercounting, authorization revocation, checkpoint durability, directory publication, and sync reconciliation.

The baseline suite is green: **357 tests passed**, formatting passed, Clippy passed, and the Markdown-only and Typst-only WASM configurations compile. Those results are useful but do not exercise the failure sequences below. The original 29 passing diagnostics and the separately passing R26 follow-up intentionally assert the *observed behavior*; they establish reproduction, not correctness. R24’s probe establishes the documented fallback, not a demonstrated consistency failure.

P1 means a high-priority security, data-loss, or resource-bound failure in a supported workflow. P2 means a significant correctness, reliability, or performance defect with a narrower trigger or impact. No P0, arbitrary remote code execution, cryptographic break, or unconditional compromise is claimed.

## Scope, snapshot, and method

The review covered all production Rust modules, build/manifests, and the test organization in `crates/komodoc`, `crates/engine`, and `crates/text`, with deeper test reading and probes around risky boundaries. Supporting frontend and specification code was consulted to understand caller contracts; this is not a full frontend review.

| Crate | Rust files at snapshot | Rust lines including tests | Review emphasis |
|---|---:|---:|---|
| `komodoc` | 52 | 29,221 | HTTP/WebSocket authorization, CLI, CRDT sessions, storage/leases, history, quotas, assets, exports, retention |
| `engine` | 7 | 1,756 | Native/WASM parity, Markdown asset resolution, Typst file access, diagnostics and ABI |
| `text` | 2 | 1,082 | UTF-16 offsets, diff application, bounded Myers diff, conflict clustering and three-way merge |

The worktree was clean when the review started. During the review, concurrent edits appeared in `room.rs`, `server.rs`, `tests/serve.rs`, and frontend files. I inspected the Rust diff: it exposes directory paths in document metadata, updates the `tree()` comment, and adds a corresponding assertion. It does not address the findings below. Those edits were preserved and were subsequently committed independently as `5a29aef86d1dcf005d0c18342ce1b1a844455481`. The Rust diff between that commit and the review snapshot was checked again before finishing. Diagnostics ran on a copy whose production `crates/` files were verified byte-for-byte against the stated commit. The source links below point to the working-tree line positions at report generation; server references after the concurrent metadata change include its six-line offset.

Method: read the implementation, trace inputs through authorization and persistence, inspect the relevant tests, then introduce deterministic storage faults and scheduling barriers only in a temporary copy. Concurrency probes use notifications at specific blob-store operations rather than hoping a race appears under load. Filesystem probes use disposable directories. Network probes use loopback fixtures; no production endpoint, bucket, user credential, or real private file was used.

## Findings index

| ID | Priority | Finding | Evidence |
|---|---|---|---|
| R01 | P1 | [Revoking access does not revoke an existing WebSocket](#r01) | Reproduced |
| R02 | P1 | [Each server keeps an indefinitely stale authorization index](#r02) | Reproduced |
| R03 | P1 | [The CLI sends one deployment’s cached bearer token to another deployment](#r03) | Code inspection |
| R04 | P2 | [A transient signing-key read failure silently replaces the deployment’s key](#r04) | Reproduced |
| R05 | P1 | [Publishing a relative directory can include files excluded by .gitignore](#r05) | Reproduced |
| R06 | P2 | [Directory publishing follows external symlinks without cycle detection](#r06) | Reproduced |
| R07 | P1 | [Checkpoint completion can clear the dirty flag of an edit it never persisted](#r07) | Reproduced |
| R08 | P1 | [A failed manifest write is skipped when unchanged content is retried](#r08) | Reproduced |
| R09 | P2 | [A successful concurrent label can be overwritten by an older checkpoint snapshot](#r09) | Reproduced |
| R10 | P1 | [Initial publish returns 201 even when no source has been saved](#r10) | Reproduced |
| R11 | P1 | [Directory creation silently succeeds with missing figures and chapters](#r11) | Reproduced |
| R12 | P1 | [Republishing a directory updates only the existing main text](#r12) | Reproduced |
| R13 | P1 | [Sync reconnection interprets unchanged local text as deletion of remote edits](#r13) | Reproduced |
| R14 | P2 | [Replacing one ordinary emoji with another panics inside Yrs](#r14) | Reproduced |
| R15 | P1 | [An unreadable retained history tree causes its assets to be garbage-collected](#r15) | Reproduced |
| R16 | P1 | [A corrupt saved session is reopened writable and can be overwritten from an empty base](#r16) | Reproduced |
| R17 | P2 | [Returning to a previous tree leaves the current history/index head on the wrong revision](#r17) | Reproduced |
| R18 | P1 | [CRDT metadata and unsupported values bypass the document byte ceiling](#r18) | Reproduced |
| R19 | P2 | [File-count admission both permits oversized batches and rejects valid small directories](#r19) | Reproduced |
| R20 | P1 | [Routine session persistence removes assets and renderings from quota accounting](#r20) | Reproduced |
| R21 | P1 | [History retention deletes trees but retains every obsolete text blob](#r21) | Reproduced |
| R22 | P2 | [Concurrent asset uploads exceed the aggregate asset limit](#r22) | Reproduced |
| R23 | P2 | [A room declared read-only still accepts and relays source edits](#r23) | Reproduced |
| R24 | Decision | [Decide whether object-level conditional writes are the intended lease fallback](#r24) | Reproduced |
| R25 | P2 | [Multipart uploads inherit a hidden 2 MiB ceiling and omit figure capacity from the explicit limit](#r25) | Reproduced |
| R26 | P2 | [Idle ticks repeat tree work and defer the next deliberate checkpoint](#r26) | Reproduced |
| R27 | P2 | [Changing the main file leaves source-format metadata stale](#r27) | Reproduced |
| R28 | P2 | [Double-encoded parent segments escape the configured LaTeX mirror URL prefix](#r28) | Reproduced |
| R29 | P2 | [Markdown image filenames containing spaces are not resolved to uploaded assets](#r29) | Reproduced |
| R30 | P2 | [Response-to-reviewers export omits every reply to a figure comment](#r30) | Reproduced |
| R31 | P2 | [The CLI cannot export or destroy the caller’s own private document](#r31) | Code inspection |
| R32 | P2 | [S3 signing double-encodes object paths that already contain percent escapes](#r32) | Code inspection |
| R33 | P2 | [The bearer-token cache has no eviction or capacity limit](#r33) | Code inspection |
| R34 | P2 | [Removing a slow peer from the room does not close its WebSocket](#r34) | Code inspection |
| R35 | P2 | [Loading one cold room holds the global room-map lock across storage I/O](#r35) | Code inspection |

## Detailed findings

<a id="r01"></a>

### R01 — P1: Revoking access does not revoke an existing WebSocket

**Source:** [komodoc/src/server.rs:1009](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1009), [komodoc/src/server.rs:1038](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1038), [komodoc/src/server.rs:2391](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:2391).

**Evidence:** reproduced by `review_revoked_socket_can_read_and_write`.

Authorization is resolved at the handshake and captured as `who`, `author`, and `is_owner`. The socket loop continues using those values; sharing changes neither refresh them nor close affected connections. The room's `may_edit` flag is equally static. Removing an editor, making a document private, expiring a link, or transferring ownership therefore does not establish the same access boundary for an existing connection as for a new HTTP request.

**Observed:** Alice published a private document, granted Bob editor access, and Bob connected. After Alice revoked Bob, Bob's REST request returned 404, but his existing socket answered `y-open` with the private state and accepted a `y-update` changing the source to `REVOKED WRITER`. Revocation is defeated for both confidentiality and integrity. Expiry/transfer are the same code path inference; they were not separate end-to-end probes.

**Correction:** associate connections with a document permission version and credential expiry; invalidate and close them when access changes, and reauthorize mutations against current policy. Revocation must stop outgoing broadcasts as well as incoming edits. **Regression:** keep sockets open across grant removal, link expiry, visibility changes, and transfer; verify no later state, comment, or update reaches an unauthorized peer.

<a id="r02"></a>

### R02 — P1: Each server keeps an indefinitely stale authorization index

**Source:** [komodoc/src/store.rs:478](/home/vincent/repos/komodoc/crates/komodoc/src/store.rs:478), [komodoc/src/store.rs:629](/home/vincent/repos/komodoc/crates/komodoc/src/store.rs:629), [komodoc/src/store.rs:705](/home/vincent/repos/komodoc/crates/komodoc/src/store.rs:705).

**Evidence:** reproduced by `review_index_conflict_stays_stale`.

`Store::get` and `list` only consult the index loaded into that process. Ordinary modifications roll back on a conditional-write conflict but do not reload the winning index. The special retry/reload inside `put` does not repair normal reads or modifications. Thus two server instances sharing storage do not converge on visibility, ownership, grants, or quota bookkeeping.

**Observed:** two `Store` instances opened the same storage. The first changed a document to private. The second still reported the earlier visibility, and two consecutive modifications failed against its obsolete version. An HTTP server using that second index can continue authorizing public reads after the first server has privatized the document. Room leases do not fix this: they coordinate room writes, not the shared authorization index.

**Correction:** define and implement cache coherence for security metadata: reload on conflicts, retry an authorized mutation against the new value, and ensure permission reads observe revocations across instances. Alternatively, explicitly enforce a single active server until that exists. **Regression:** two live HTTP instances behind shared storage, including revocation/transfer on one followed by reads and writes on the other.

<a id="r03"></a>

### R03 — P1: The CLI sends one deployment’s cached bearer token to another deployment

**Source:** [komodoc/src/cli.rs:38](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:38), [komodoc/src/cli.rs:56](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:56), [komodoc/src/cli.rs:99](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:99), [komodoc/src/cli.rs:489](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:489).

**Evidence:** high-confidence code inspection; no dedicated end-to-end reproduction.

`login` stores only the token in one global `komodoc/token` file. `stored_token()` takes no server argument, and publish/sync and other commands pass that value to whichever `--server` or server environment setting is selected. Signing in to trusted server A and then publishing to server B therefore sends A's reusable bearer credential to B. B need not accept the token to capture and replay it against A.

This is a direct data-flow finding, not a claim that an actual token was exposed during this review. No personal token file was read and no credential was sent to an external service. The condition is using multiple deployments, which the CLI already supports.

**Correction:** cache credentials by a normalized, trusted server origin and select them only for that origin. Store the issuing origin alongside newly received credentials; migrate the old unscoped format without silently forwarding it elsewhere. Handle explicitly supplied `KOMODOC_TOKEN` separately and document its destination semantics. **Regression:** two loopback servers with synthetic tokens; logging in to A and publishing to B must not attach A's cached Authorization header.

<a id="r04"></a>

### R04 — P2: A transient signing-key read failure silently replaces the deployment’s key

**Source:** [komodoc/src/auth.rs:383](/home/vincent/repos/komodoc/crates/komodoc/src/auth.rs:383).

**Evidence:** reproduced by `review_session_key_transient_read_rotates_key`.

`session_key` treats an unavailable or invalid existing key like an absent key, generates a replacement, and stores it with unconditional `put`. A temporary GET error followed by a successful PUT permanently changes the signing identity of the deployment. Concurrent initial startups can also generate different keys and leave one process signing with the losing value.

**Observed:** a valid persisted key was created, then a wrapper failed only its read. Calling `session_key` returned a different key and overwrote the original. Later normal reads returned the replacement. Existing signed sessions, device credentials, and visitor proofs become unverifiable; a process retaining the old key disagrees with a newly started one. This is an availability and identity-continuity failure, not a demonstrated confidentiality leak; it is now P2.

**Correction:** generate only on an explicit NotFound, refuse malformed existing key material, and fail startup on other read errors. Create with compare-and-set against absence, and load the winner on a creation conflict. **Regression:** read failure with successful write availability must leave the original bytes untouched; concurrent initializations must all return the same key.

<a id="r05"></a>

### R05 — P1: Publishing a relative directory can include files excluded by .gitignore

**Source:** [komodoc/src/cli.rs:216](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:216), [komodoc/src/cli.rs:270](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:270).

**Evidence:** reproduced by `review_relative_directory_gitignore_is_not_honored`.

`files_under` passes paths prefixed by the publish root into the ignore predicate. `git_ignores` also sets `git -C` to that root, then passes the prefixed path unchanged to `git check-ignore`. With a relative root such as `paper`, Git evaluates `paper/private.txt` relative to `paper`, rather than evaluating `private.txt` there. Root-anchored ignore patterns are missed.

**Observed:** a temporary Git repository contained `/private.txt` in `.gitignore`. Listing it through a relative directory path included `private.txt` in the publish list. An unanchored `private.txt` pattern can accidentally match both paths and conceal this bug, which is why the diagnostic uses the anchored form. The trigger is conditional, but relative publish roots and root-anchored ignores are ordinary usage; silently publishing an explicitly excluded file remains P1 because the exposure can be irreversible.

**Correction:** pass a path relative to the Git command's working directory, or canonicalize both root and candidate consistently. Use `--` to end Git options and consider batching ignore queries. **Regression:** absolute and relative roots, anchored and nested patterns, ignored directories, filenames beginning with `-`, and paths containing spaces. Treat exclusions as a publication boundary because ignored files can contain private data, not only build products.

<a id="r06"></a>

### R06 — P2: Directory publishing follows external symlinks without cycle detection

**Source:** [komodoc/src/cli.rs:229](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:229), [komodoc/src/render.rs:184](/home/vincent/repos/komodoc/crates/komodoc/src/render.rs:184), [komodoc/src/render.rs:271](/home/vincent/repos/komodoc/crates/komodoc/src/render.rs:271).

**Evidence:** `review_typst_reader_follows_external_symlink` reproduces following external symlinks. Missing cycle detection is established by inspection; no timed cycle stress test was run.

Directory traversal follows directory symlinks recursively without tracking visited directories. The native rendering readers reject lexical parent components but read joined paths without checking resolved containment. An in-root symlink can therefore include content outside the lexical publication root.

**Observed:** a source directory contained a `linked` symlink to a separate temporary directory. Typst `#read("linked/private.txt")` rendered the external text, and `files_under` returned `linked/private.txt` as a file to publish. No actual personal files were accessed.

**Revised interpretation:** this is a local CLI file-selection policy issue, not a demonstrated remote sandbox escape. Native rendering runs on user-selected local input, operator-controlled seed examples, or an export directory assembled from texts; ordinary uploads cannot create filesystem symlinks. Following a user's intentional symlink can be valid behavior. The concrete robustness defect is missing cycle detection: traversal can revisit ancestors and repeatedly enumerate the same content until filesystem path/symlink limits intervene. Accidental external publication depends on what the CLI promises to include and what the user expects, so it does not justify the original P1 security framing.

**Correction:** define whether publication follows external symlinks, make that behavior visible, and prevent revisiting the same resolved directory. Require resolved containment only if that is the intended policy, rather than treating it as an established sandbox guarantee. **Regression:** intentional internal/external symlinks, ancestor cycles, and several symlinks to the same directory; ensure termination without repeated publication of the same tree.

<a id="r07"></a>

### R07 — P1: Checkpoint completion can clear the dirty flag of an edit it never persisted

**Source:** [komodoc/src/room.rs:1500](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1500), [komodoc/src/room.rs:1530](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1530).

**Evidence:** reproduced by `review_checkpoint_race_drops_dirty_edit`.

A checkpoint snapshots the tree, later encodes session bytes, writes those bytes, awaits the parent-tree read, and finally reacquires state and unconditionally sets `session.dirty = false`. These are different critical sections. An edit that arrives after the session encoding but before that final assignment is not present in storage, yet its dirty flag is cleared by the older checkpoint.

**Observed:** after reopening a room, the probe checkpointed B and paused the parent's storage GET after the session write. It then changed the live source to C, resumed the checkpoint, and inspected both states. Memory contained C, storage contained B, `dirty` was false, and `persist()` returned false. A subsequent crash can lose C even though no persistence failure was reported. This is not merely a stale history display.

**Correction:** track a monotonically increasing document generation or state vector with each snapshot. Clear dirty only if the persisted snapshot still covers the current generation; preserve newer dirty work. Serialize checkpoint transactions separately from short edit locks. **Regression:** inject edits at every await between snapshot, session write, parent lookup, manifest write, and completion, then reload and verify every acknowledged edit.

<a id="r08"></a>

### R08 — P1: A failed manifest write is skipped when unchanged content is retried

**Source:** [komodoc/src/room.rs:1455](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1455), [komodoc/src/room.rs:1533](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1533), [komodoc/src/room.rs:1580](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1580), [komodoc/src/room.rs:1623](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1623).

**Evidence:** reproduced by `review_failed_manifest_write_never_retries`.

Checkpoint state is advanced in memory before the manifest's conditional write succeeds. On a manifest write error, the new checkpoint remains in `state.manifest` and in session checkpoint bookkeeping. Retrying with unchanged content hits `manifest.has(sha)` and returns success without attempting to persist the missing manifest entry.

**Observed:** failing the history-index write for B made the first checkpoint return an error. After storage recovered, the retry returned B successfully, but loading the manifest from storage still found no B. The session bytes/tree can exist while the user-visible history and restore inventory omit them. The recovery comment also promises to use the index's latest SHA, but `load_session` initializes `last_checkpoint` from the manifest, so a restart does not necessarily supply the missing index head to `repair`.

**Correction:** distinguish staged and committed manifest state; publish the committed in-memory state only after success, or retain an explicit retryable transaction. Reconcile the persisted index, manifest, and existing objects during recovery. Do not let the content-deduplication early return bypass unfinished bookkeeping. **Regression:** fail each checkpoint write in turn, retry unchanged content, restart, and require the same complete history from memory and storage.

<a id="r09"></a>

### R09 — P2: A successful concurrent label can be overwritten by an older checkpoint snapshot

**Source:** [komodoc/src/room.rs:1533](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1533), [komodoc/src/room.rs:1580](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1580), [komodoc/src/room.rs:1738](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1738).

**Evidence:** reproduced by `review_concurrent_label_is_lost_by_checkpoint`.

A checkpoint clones the manifest and releases the room lock while calculating allowance and recording the document index. `label` can successfully update and persist that manifest during the gap. When the checkpoint resumes, it writes its earlier clone using the latest `manifest_version`, so the compare-and-set succeeds while discarding the label. The storage version protects against other processes, but does not validate the freshness of a snapshot produced inside this process.

**Observed:** the diagnostic paused B's index swap after its manifest snapshot, labeled A as `accepted label`, and waited for the label operation to return success. Resuming B removed that label from both memory and the persisted manifest. Because labels affect history retention, the loss also removes the user's retention preference.

**Correction:** serialize all manifest mutations through one transaction path, or merge changes against the current manifest/version and retry a stale snapshot. **Regression:** checkpoints overlapping labels, labels overlapping each other, and multiple checkpoints. Assert that every operation returning success remains represented after a reload.

<a id="r10"></a>

### R10 — P1: Initial publish returns 201 even when no source has been saved

**Source:** [komodoc/src/server.rs:1360](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1360), [komodoc/src/server.rs:1395](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1395).

**Evidence:** reproduced by `review_creation_reports_success_without_saved_source`.

Creation first writes the document index. The source lives in the room and its first checkpoint; `Store::put` no longer writes a separate source object. If `room.checkpoint` fails, the creation handler logs a warning, falls back to the earlier SHA, and still returns HTTP 201 with a document URL. That response can describe a document whose only copy of the submitted source is in RAM.

**Observed:** a storage wrapper rejected `history/` writes while allowing the document index. Publishing `ONLY IN RAM` returned 201 and the running room showed that source. No session object existed. A fresh server over the same storage read an empty document. The probe restarted before a later background retry could happen, which is precisely the crash window the success response conceals.

**Correction:** only return a successful creation after a durable source/session commit. Propagate storage failure and reconcile or roll back incomplete index entries; use an idempotent publication transaction if retries can cross failures. **Regression:** fail text-blob, tree, session, and manifest writes separately and verify that a reported successful publish always survives immediate restart.

<a id="r11"></a>

### R11 — P1: Directory creation silently succeeds with missing figures and chapters

**Source:** [komodoc/src/server.rs:1385](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1385), [komodoc/src/server.rs:1761](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1761).

**Evidence:** reproduced by `review_directory_accepts_partial_failure`.

`fill_directory` can fail after inserting only part of the upload. Its caller logs the error but still checkpoints the partial tree and returns 201. A non-UTF-8 secondary text is skipped without even returning an error. Per-asset size validation occurs during insertion rather than as a complete preflight, although the aggregate limits were checked earlier.

**Observed:** with `max_asset=10`, a directory containing an 11-byte figure followed by a chapter returned 201. The saved tree contained only the main file: the rejected figure and the later chapter were both absent. The response provides no list of omitted files. This differs from R10: storage can be healthy and the resulting partial publication can be fully durable while still losing requested content.

**Correction:** validate every file, encoding, individual limit, aggregate limit, and owner allocation before mutating the room. Commit a complete directory or return an explicit failure; if partial upload is a product requirement, the API and CLI must expose every omission and require a deliberate retry flow. **Regression:** bad secondary UTF-8, oversized middle asset, and storage failure at each file position must never produce an unqualified successful complete publication.

<a id="r12"></a>

### R12 — P1: Republishing a directory updates only the existing main text

**Source:** [komodoc/src/server.rs:1330](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1330), [komodoc/src/server.rs:1420](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1420).

**Evidence:** reproduced by `review_directory_republish_ignores_chapters`.

The new-publication branch fills `parsed.files`, but the existing-document branch calls `edit_into_session`, which only runs `set_source` on `parsed.source`. It does not apply the parsed directory, its chosen main path, additions, changed secondary files, or removals. The complete multipart upload is accepted and a normal 201 response is returned.

**Observed:** an initial directory had `main.md` and `chapter.txt=old`. Republishing the same slug with `chapter.txt=NEW` and `new.txt=ADDED` returned 201, but the chapter remained `old` and `new.txt` did not exist. The uploaded work never enters the shared session or history.

**Correction:** implement directory reconciliation by stable file identity/path with explicit semantics for deletion and main-file changes, then apply it as one coherent session operation. Validate it before broadcasting, and retain concurrent edits according to the documented merge policy. **Regression:** republish changed/added/deleted text and asset files, renamed main files, and a directory while another editor changes an unaffected chapter.

<a id="r13"></a>

### R13 — P1: Sync reconnection interprets unchanged local text as deletion of remote edits

**Source:** [komodoc/src/sync.rs:377](/home/vincent/repos/komodoc/crates/komodoc/src/sync.rs:377).

**Evidence:** reproduced by `review_sync_reconnect_rolls_back_remote`.

`joined()` resets `self.base` to the newly received remote text before reconciling the local file on every join. On reconnection the previous common base is still meaningful, but replacing it makes remote additions look like changes that the local file deliberately removed. The outgoing CRDT state then carries those removals back to the server.

**Observed:** the local file and session initially contained `alpha beta`. After the remote state advanced to `alpha REMOTE beta`, receiving a reconnect state while the local file remained untouched produced `alpha beta` in the client's document. The new remote word was deleted. Existing convergence tests demonstrate exchange of CRDT updates; they do not establish that the client's merge inputs describe the user's intentions correctly.

**Correction:** distinguish first attachment from reconnection and preserve the last common disk/session base. Reconcile local and remote changes against that common ancestor before updating it. **Regression:** reconnect after remote-only edits, local-only edits, both sides editing separate passages, and conflicting edits; an unchanged local file must not erase remote work.

<a id="r14"></a>

### R14 — P2: Replacing one ordinary emoji with another panics inside Yrs

**Source:** [komodoc/src/session.rs:559](/home/vincent/repos/komodoc/crates/komodoc/src/session.rs:559).

**Evidence:** reproduced by `review_replacing_emoji_splits_surrogate_pair`.

`edit_text` computes its common prefix and suffix over raw UTF-16 code units. Distinct non-BMP characters can share a high surrogate or low surrogate. The resulting replacement boundary can split a surrogate pair, even though both input Rust strings contain valid Unicode. `String::from_utf16_lossy` on the inserted fragment does not make an invalid deletion boundary safe.

**Observed:** calling `replace_text` with 😀 and then 😁 panicked in `yrs-0.27.4/src/types/text.rs:845`: `Couldn't remove 1 elements from an array. Only 0 of them were successfully removed.` The diagnostic catches the unwind to record the defect; production does not catch it here. This path is used for ordinary source replacement and text insertion helpers, so publish/restore-style operations can fail on valid document content. This demonstrates a task panic, not evidence that the whole Tokio process terminates or that previously durable source is lost. P2 better matches that demonstrated impact.

**Correction:** choose edit boundaries on Unicode scalar boundaries, then translate those boundaries into UTF-16 offsets for Yrs. Ensure both retained prefix and suffix avoid splitting a pair. **Regression:** emoji-to-emoji substitutions sharing either surrogate, adjacent emoji, insertions/deletions around them, and round trips through browser Yjs.

<a id="r15"></a>

### R15 — P1: An unreadable retained history tree causes its assets to be garbage-collected

**Source:** [komodoc/src/room.rs:2127](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:2127), [komodoc/src/room.rs:2158](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:2158).

**Evidence:** reproduced by `review_failed_tree_read_prunes_retained_asset`.

Asset pruning builds a set of referenced digests by loading every retained history tree. A failed `load_tree` is silently skipped, after which pruning proceeds as though that checkpoint referenced no assets. If an asset is absent from the live document and old enough to clear the grace period, a temporary read error can therefore turn into a successful permanent deletion of required historical content.

**Observed:** a retained checkpoint referenced `fig.png`. The live document later removed that reference. With grace set to zero for determinism, the probe failed the retained tree's GET during another checkpoint. The checkpoint remained in the manifest, but its asset object was deleted. Restoring the retained version can no longer restore its figure.

**Correction:** garbage collection must require a complete, successfully read reachability set. Abort the destructive sweep if any retained tree is unreadable; retain objects conservatively and retry later. **Regression:** fail each retained-tree GET with NotFound, malformed data, and transient storage errors; no possibly referenced asset may be deleted. Include a concurrent asset-naming case as well.

<a id="r16"></a>

### R16 — P1: A corrupt saved session is reopened writable and can be overwritten from an empty base

**Source:** [komodoc/src/room.rs:620](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:620), [komodoc/src/room.rs:674](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:674).

**Evidence:** reproduced by `review_corrupt_session_overwritten_on_load`.

When `get_versioned` successfully returns a session object but applying its Yrs update fails, `load_session` logs the failure and continues with the partially initialized/empty document. It retains the corrupt object's version and does not make the room read-only or reconstruct it from a valid checkpoint. The next mutation can thus conditionally overwrite the original object with a new empty-based state.

**Observed:** replacing a saved session with `[255]` left an existing valid checkpoint intact. Reopening the room produced empty source with `read_only=false`; setting new text and persisting replaced the session successfully. The checkpoint was not used for recovery. Unlike a transient GET failure, this path retains an ETag that lets the subsequent overwrite succeed; those cases should not be conflated.

**Correction:** load/decode into a temporary document, install it only after validation, and either recover from a verified checkpoint or refuse writes pending recovery. Preserve the unreadable object for diagnosis. Treat unreadable manifests similarly instead of silently defaulting them. **Regression:** truncated/malformed session updates with valid history, malformed manifests with valid sessions, and storage read failures.

<a id="r17"></a>

### R17 — P2: Returning to a previous tree leaves the current history/index head on the wrong revision

**Source:** [komodoc/src/room.rs:1455](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1455), [komodoc/src/room.rs:1034](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1034).

**Evidence:** reproduced by `review_revisiting_checkpoint_leaves_wrong_head`.

The content-deduplication branch returns an existing tree SHA and updates only `session.last_checkpoint` and its timestamp. It does not change the document index head, manifest ordering/current-head representation, or `last_tree`. Consumers that use `manifest.latest()` therefore keep treating another revision as current.

**Observed:** checkpoint A, checkpoint B, then restore the text to A and checkpoint again. The operation returned A, but both `Store::get().sha` and `manifest.latest().sha` remained B. Comment/reply bookkeeping derives revision information from the latest manifest checkpoint, so a new annotation can be associated with B while the author is looking at A. Follow-on change summaries can also use the wrong parent tree.

**Correction:** distinguish a content-addressed tree from a chronological revision event/current-head pointer. Reusing immutable content must still move the current head and preserve the intended timeline semantics. **Regression:** A→B→A with comments, labels, restore, current-source metadata, and a subsequent C checkpoint, including a restart between operations.

<a id="r18"></a>

### R18 — P1: CRDT metadata and unsupported values bypass the document byte ceiling

**Source:** [komodoc/src/session.rs:426](/home/vincent/repos/komodoc/crates/komodoc/src/session.rs:426), [komodoc/src/session.rs:487](/home/vincent/repos/komodoc/crates/komodoc/src/session.rs:487).

**Evidence:** reproduced by `review_metadata_bypasses_byte_limit`.

The exact admission measurement counts text bodies and selected keys, but not metadata values, path-map values, arbitrary non-YText values, or other roots in the Yrs document. When the cheap bound exceeds the ceiling, the scratch-copy path can therefore accept a large update whose payload lives outside the measured fields. The update is still applied to the real CRDT and included in subsequent full-state encodings.

**Observed:** a 200 KB metadata string was admitted to an empty document with a 1 KB byte ceiling. This bypass occurred on the supposedly exact fallback path. The socket's 1 MiB message limit bounds individual frames, not the cumulative state stored across frames. A permitted editor can consume memory, persistence bandwidth, and storage beyond the configured document bound.

**Correction:** validate the allowed Yrs schema and bound every retained value/root, including path lengths and metadata. Maintain a separate upper bound on encoded/retained CRDT state if logical-text quotas intentionally exclude CRDT overhead. **Regression:** large metadata/path values, unexpected nested maps/arrays, additional roots, and repeated updates whose visible text stays small.

<a id="r19"></a>

### R19 — P2: File-count admission both permits oversized batches and rejects valid small directories

**Source:** [komodoc/src/session.rs:426](/home/vincent/repos/komodoc/crates/komodoc/src/session.rs:426), [komodoc/src/session.rs:487](/home/vincent/repos/komodoc/crates/komodoc/src/session.rs:487).

**Evidence:** reproduced by `review_file_count_fast_path_and_double_count`.

The cheap admission branch only asks whether the document's *old* key count is below `max_files`. It does not bound how many files the incoming update adds. The exact measurement, meanwhile, counts both a text file's `files` entry and its `paths` entry as separate files. These are independent mistakes in the same file-limit invariant.

**Observed:** a single update creating 20 text files was accepted with `max_files=5`. A document containing only three legitimate text files then rejected an otherwise valid no-op update as `TooMany` under the same ceiling. Under defaults, the latter affects documents well before the advertised text-file capacity.

**Correction:** count logical text/asset files once and measure or conservatively bound the post-update count even when the byte bound is cheap. A below-limit current count is not proof that a batch will remain below the limit. **Regression:** one update adding many files, text/asset mixtures, deleting at the limit, and no-op or text-only edits when a valid document is near its limit.

<a id="r20"></a>

### R20 — P1: Routine session persistence removes assets and renderings from quota accounting

**Source:** [komodoc/src/room.rs:1356](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1356), [komodoc/src/room.rs:1393](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1393).

**Evidence:** reproduced by `review_persist_forgets_asset_and_rendering_charges`.

`persist()` records `session_size + manifest.bytes()` as the document's complete size. Other accounting paths also include `assets_bytes()` and `renderings_bytes()`. Because `record_history` replaces the stored total, an ordinary text edit followed by background persistence drops all separately stored asset and rendering charges from the index used for owner/deployment admission.

**Observed:** after storing 100,000 asset bytes and 100,000 rendering bytes, a short text edit followed by `persist()` reduced the recorded size to 114 bytes while all 200,000 object bytes remained stored. A later full checkpoint may restore some accounting, but the normal persistence path repeatedly reopens the undercounted interval.

**Correction:** centralize physical-byte accounting in one routine used by every mutation and include each object category consistently. Make quota decisions against an authoritative total or reservation scheme, not a partially updated cached total. **Regression:** upload assets and renderings, edit/persist, restart, prune, and compare the index charge against the intended physical object inventory throughout the sequence.

<a id="r21"></a>

### R21 — P1: History retention deletes trees but retains every obsolete text blob

**Source:** [komodoc/src/room.rs:1479](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1479), [komodoc/src/room.rs:1590](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1590), [komodoc/src/history.rs:169](/home/vincent/repos/komodoc/crates/komodoc/src/history.rs:169).

**Evidence:** reproduced by `review_shed_history_leaks_text_blobs`.

Checkpoints store text separately under `history/<slug>/blobs/<digest>`. When retention sheds checkpoints, cleanup only deletes `checkpoint_key` objects and prunes assets/renderings. There is no corresponding reachability sweep of the text-blob namespace. The manifest's charged history bytes shrink even though obsolete text bodies remain in the bucket/filesystem.

**Observed:** with `history_max=1`, creating A, B, C, and D left one checkpoint but all four distinct text blobs. Repeated replacement of a large document accumulates physical storage without an equivalent retained-history charge, so the retention/quota mechanism does not bound its principal historical payload. Deleting the entire document can reclaim the namespace; normal retention cannot.

**Correction:** collect text digests reachable from all retained trees, and safely delete unreferenced blobs after successful manifest commit, with concurrent-writer/grace protection. Charge deduplicated objects once rather than summing logical references. **Regression:** repeated distinct revisions under a tiny history cap, shared bodies across several files/checkpoints, labeled history, failed tree reads, and interrupted garbage collection.

<a id="r22"></a>

### R22 — P2: Concurrent asset uploads exceed the aggregate asset limit

**Source:** [komodoc/src/room.rs:1868](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1868), [komodoc/src/room.rs:1909](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1909).

**Evidence:** reproduced by `review_concurrent_asset_admission_exceeds_limit`.

`put_asset` checks the current aggregate size under the room lock, releases it before lease/storage awaits, then inserts the uploaded size afterward. Distinct concurrent uploads can all pass against the same old total. HTTP-level checks also precede awaited operations and do not reserve bytes.

**Observed:** under a 15-byte aggregate ceiling, the probe paused the storage write for a 10-byte asset, uploaded a different 10-byte asset successfully, then resumed the first. Both calls succeeded and the aggregate became 20. The same pattern scales to ordinary multi-megabyte concurrent uploads; this is not dependent on malformed CRDT messages.

**Correction:** reserve capacity atomically before releasing the lock and release the reservation on failure, or serialize the admission/write/accounting transaction. Deduplicate concurrent requests for the same digest and coordinate owner-level reservations across documents where those quotas apply. **Regression:** concurrent distinct and duplicate uploads, failed writes releasing capacity, and simultaneous uploads to different documents owned by one account.

<a id="r23"></a>

### R23 — P2: A room declared read-only still accepts and relays source edits

**Source:** [komodoc/src/room.rs:816](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:816), [komodoc/src/room.rs:1280](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1280), [komodoc/src/room.rs:1356](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1356).

**Evidence:** reproduced by `review_read_only_room_relays_edits`.

Failure to acquire the room lease sets its read-only flag, but `receive_update` does not inspect that flag or establish ownership before applying the update. It can return `Applied::Relay`, and the server then broadcasts that update. Only later persistence refuses to save it. The same absence of a guard affects direct in-memory source mutation helpers.

**Observed:** a second `RoomSet` opened a room actively leased by the first. It reported read-only, accepted an authorized peer update, returned `Relay`, and exposed `UNSAVABLE EDIT` as its source. `persist()` then failed because another server held the room. This creates divergent live state on the losing server and gives its peers an editing session that cannot commit. The probe does not claim a durable acknowledgment was issued; the ack path appropriately waits for persistence.

**Correction:** reject edit attachment/mutations as soon as the server lacks ownership, and signal read-only/redirect/reconnect before applying data. If ownership is lost during a session, stop mutation and clearly preserve unsent client changes for reconnection. **Regression:** two server instances and lease loss during an active editor session.

<a id="r24"></a>

### R24 — Decision: Decide whether object-level conditional writes are the intended lease fallback

**Source:** [komodoc/src/blob.rs:482](/home/vincent/repos/komodoc/crates/komodoc/src/blob.rs:482), [komodoc/src/blob.rs:529](/home/vincent/repos/komodoc/crates/komodoc/src/blob.rs:529), [komodoc/src/blob.rs:570](/home/vincent/repos/komodoc/crates/komodoc/src/blob.rs:570), [komodoc/src/room.rs:837](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:837).

**Evidence:** `review_lease_error_reports_held` confirms that a failed lock read returns `held=true` with a new timestamp. It does not demonstrate an incorrect overwrite, lost committed update, or broken history under the intended fallback model.

The implementation explicitly chooses continued availability when the lease store is unreadable and names conditional writes on the room's own objects as the actual enforcement. `write_owned` does compare-and-set on the saved object version and makes the room read-only after a conflict. My original P1 classification did not give that stated rationale enough weight. This is a design decision requiring validation, not a confirmed P1 defect merely because it is deliberate fail-open lease acquisition.

**Remaining question:** is the guarantee object-level conflict detection, or exclusive ownership across a multi-object checkpoint and garbage-collection transaction? Object-level CAS can reject stale overwrites but does not by itself prove the latter. Conversely, a probe that only makes `held` true does not prove that the latter is violated in an actual execution. This report has not established that counterexample.

**Decision and validation:** document the intended guarantee and supported storage failure model. Exercise two holders under partial lock-store failure, with interleaved session, comments, manifest, and cleanup operations, and assert that acknowledged work and reachable objects survive. If exclusive verified ownership is required, do not extend the verified lifetime on an error. If CAS fallback is intentional, distinguish provisional permission to attempt a write from a successfully acquired lease, and validate every mutation/deletion path against that contract. Schedule this decision behind the reproduced cost and data-loss defects.

<a id="r25"></a>

### R25 — P2: Multipart uploads inherit a hidden 2 MiB ceiling and omit figure capacity from the explicit limit

**Source:** [komodoc/src/server.rs:1478](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1478), [komodoc/src/server.rs:1499](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1499).

**Evidence:** reproduced by `review_directory_body_limit_ignores_figure_allowance`.

The handler wraps the body in its own `Limited` reader but then invokes Axum's multipart extractor without overriding its default body limit. The extractor retains its 2 MiB default. Independently, the explicit aggregate ceiling is `max_document + MULTIPART_SLACK`, even though a directory is allowed a separate `max_assets` budget. Fixing only the extractor still rejects valid text-plus-figure uploads.

**Observed:** a 3 MiB figure was rejected with HTTP 400 on default configuration, despite the individual figure allowance being 8 MiB. The response was `bad upload`, rather than a useful size-limit diagnostic. This is a native HTTP multipart probe against the real handler, not an inference from the CLI.

**Correction:** set one consistent extractor/request ceiling derived safely from document bytes, aggregate assets, bounded multipart metadata, and configured file count. Preserve individual/category checks and map actual limit exhaustion to 413 even without Content-Length. **Regression:** valid payloads just over 2 MiB, a large permitted figure, text plus several assets, chunked requests, and exact configured boundaries.

<a id="r26"></a>

### R26 — P2: Idle ticks repeat tree work and defer the next deliberate checkpoint

**Source:** [komodoc/src/room.rs:2222](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:2222), [komodoc/src/room.rs:550](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:550).

**Evidence:** reproduced by `review_quiet_room_never_reports_idle`; the follow-up `review_idle_tick_defers_next_changed_checkpoint` independently confirms checkpoint deferral.

`tick` compares `digest_of(main_text)` with `last_checkpoint`, whose identity is now a digest of the whole directory tree. A clean room consequently keeps entering the quiet-checkpoint branch. Once the quiet interval has elapsed, the once-per-second sweeper reconstructs/hashes the tree and runs a no-op checkpoint repeatedly. Its duplicate-content return avoids most storage writes but refreshes `last_checkpoint_at` and prevents idle reporting.

**Additional observation:** the follow-up starts with an old checkpoint timestamp, runs one no-op quiet tick, changes the source, and calls `checkpoint("cli", ...)`. It returns `None`, leaving the index at the old SHA. Thus an edit to a long-idle document unnecessarily enters the 30-second defer window; the existing-document publish handler uses the previous SHA in its 201 response. The probe directly tests room deferral/index state; the HTTP response consequence follows from the handler's explicit `Ok(None)` branch.

This is more consequential than idle trimming alone and belongs near the front of the work list, particularly when bounding recurring work is a priority. It is not indefinite starvation after a real edit: changing the source resets its quiet timestamp, so a later tick can service the deferred checkpoint. Nor does it imply that session persistence itself waits 30 seconds; that timer is separate. Pressure-based room eviction still exists.

**Correction:** compare like identities or track a tree generation that is changed by every relevant text/path/main/asset mutation. Do not refresh the deliberate-checkpoint clock for unchanged content. Keep idle detection independent of a no-op checkpoint attempt. A direct comparison fix is small; avoid replacing it with unconditional full-tree hashing every second if reducing idle work is the objective. **Regression:** idle-room eviction, asset-only/main-file changes, no repeated tree work for an unchanged room, and immediate publish-style checkpointing after long idle with a current returned SHA.

<a id="r27"></a>

### R27 — P2: Changing the main file leaves source-format metadata stale

**Source:** [komodoc/src/room.rs:1280](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1280), [komodoc/src/room.rs:1805](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1805), [komodoc/src/session.rs:143](/home/vincent/repos/komodoc/crates/komodoc/src/session.rs:143), [komodoc/src/room.rs:2322](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:2322).

**Evidence:** reproduced by `review_changing_main_does_not_change_format`.

The main-file identity is shared CRDT metadata, but the room's separate `session.format` is initialized from the index or direct source-setting calls. A CRDT main-file change does not recompute it. Checkpoint and index bookkeeping later combine the new main path with the stale format.

**Observed:** changing the main file from Markdown to `paper.typ`, then checkpointing, produced an index entry with `main="paper.typ"` and `source_format="markdown"`. API consumers and stored checkpoint metadata receive inconsistent instructions. Some frontend rendering paths infer the format from the filename and may mask the problem; this finding does not assert that every browser preview necessarily uses the stale field.

**Correction:** derive format from the authoritative main file whenever the directory changes, or store it as one validated part of the same shared state. Keep restores and main-file renames consistent too. **Regression:** Markdown↔Typst↔LaTeX main switches through Yjs, renames that change extensions, API metadata, and restart/restore round trips. Also review `main_path_for`: its fallback for a single-file LaTeX source is `main.txt`, not `main.tex`.

<a id="r28"></a>

### R28 — P2: Double-encoded parent segments escape the configured LaTeX mirror URL prefix

**Source:** [komodoc/src/latex.rs:146](/home/vincent/repos/komodoc/crates/komodoc/src/latex.rs:146), [komodoc/src/latex.rs:218](/home/vincent/repos/komodoc/crates/komodoc/src/latex.rs:218).

**Evidence:** reproduced by `review_mirror_double_encoded_parent_escapes_base`.

`safe_path` percent-decodes once and rejects literal parent components. For `%252e%252e/private.json`, the checked path still contains `%2e%2e`. Concatenating that with the upstream base and passing it to the HTTP client's URL parser normalizes the encoded dot segment, issuing a request outside the configured prefix.

**Observed:** a mirror whose base ended in `/latex/` received `%252e%252e/private.json`; the local upstream fixture observed `/private.json`. The fixture constructed `Mirror::Upstream` directly with loopback HTTP to observe the request, while production configuration requires HTTPS. The path-normalization behavior is the same; TLS is not what supplies the missing containment check.

**Correction:** validate the final parsed URL against the allowed origin and path prefix, and reject encoded separators/dot segments that can acquire path semantics later. Prefer constructing encoded path segments rather than concatenating partially decoded strings. **Regression:** singly/doubly encoded parents and separators, literal percent filenames, and preserved valid nested mirror paths. The demonstrated escape stays on the configured upstream origin; it is not arbitrary-host SSRF.

**Scope note (second review):** `DEFAULT_MIRROR` is a dedicated host, so with the default configuration there is nothing above `/latex/` to reach. The escape matters only when `--latex` points at a path inside a bucket that also holds other objects. The `Directory` variant is unaffected: `root.join("%2e%2e/...")` names a literal directory. Low impact; fix it with the URL-prefix check, but schedule it behind the cost and data-loss items.

<a id="r29"></a>

### R29 — P2: Markdown image filenames containing spaces are not resolved to uploaded assets

**Source:** [engine/src/markdown.rs:100](/home/vincent/repos/komodoc/crates/engine/src/markdown.rs:100), [engine/src/markdown.rs:119](/home/vincent/repos/komodoc/crates/engine/src/markdown.rs:119).

**Evidence:** reproduced by `review_markdown_encoded_image_path_not_resolved`.

Comrak URL-encodes Markdown image destinations before `rewrite_images` sees the HTML. The rewriter decodes HTML entities but not URL escaping, and asks the resolver for that encoded string. Asset maps use the actual directory filename, so a valid image path with a space is absent under its encoded spelling. The output retains a relative URL rather than the uploaded asset URL/data URL.

**Observed:** rendering `![plot](<fig/my plot.png>)` with a resolver that knows `fig/my plot.png` produced `src="fig/my%20plot.png"` and never embedded the available asset. The engine-level behavior was exercised from the native review module using the real engine dependency; no browser screenshot was needed to establish the failed lookup.

**Correction:** resolve URL path components to canonical document paths consistently, preserving the distinction between path, query, fragment, and literal percent characters. Apply the same path convention in native and WASM hosts. **Regression:** spaces, Unicode, percent signs, HTML-sensitive characters, nested main files, and URLs with query/fragment components. Leave genuine external URLs unchanged.

<a id="r30"></a>

### R30 — P2: Response-to-reviewers export omits every reply to a figure comment

**Source:** [komodoc/src/export.rs:389](/home/vincent/repos/komodoc/crates/komodoc/src/export.rs:389), [komodoc/src/export.rs:423](/home/vincent/repos/komodoc/crates/komodoc/src/export.rs:423).

**Evidence:** reproduced by `review_response_omits_figure_replies`.

The figure-region branch writes the question and figure position, then executes `continue`. The common reply-rendering loop is below that branch, so none of the figure comment's replies appear in the response-to-reviewers document. The text-comment path does include them.

**Observed:** a figure question with the reply `THE AUTHOR ANSWER` produced an export containing the question and no author answer. This discards the part of the thread the response document is specifically meant to collect, without an error or omission notice. It affects the response format; the finding does not claim every other export format loses the same data.

**Correction:** limit the branch to choosing the anchor/passage description and always render the thread afterward. **Regression:** figure and text comments with multiple replies by different people, resolved/unresolved states, and empty comment bodies. Verify the response document contains every reply once and preserves its creator.

<a id="r31"></a>

### R31 — P2: The CLI cannot export or destroy the caller’s own private document

**Source:** [komodoc/src/export.rs:102](/home/vincent/repos/komodoc/crates/komodoc/src/export.rs:102), [komodoc/src/export.rs:118](/home/vincent/repos/komodoc/crates/komodoc/src/export.rs:118), [komodoc/src/cli.rs:1058](/home/vincent/repos/komodoc/crates/komodoc/src/cli.rs:1058).

**Evidence:** high-confidence code inspection; no dedicated end-to-end reproduction.

Both commands first fetch `/api/documents/<slug>` through unauthenticated `get_json`, even though identifier resolution and later write/history operations can use the cached token. Export also fetches comments with no Authorization header. On a private document, the owner receives the same 404 as an anonymous caller and the CLI exits before it reaches the authorized operation. `--yes` only skips confirmation; it does not fix the unauthenticated preflight.

This follows directly from the request construction and the private-document authorization already exercised in R1. A separate subprocess CLI test was not run because these command functions exit the process on error.

**Correction:** select the origin-scoped token once and use authenticated reads consistently throughout export/destroy. Preserve explicit anonymous behavior only for publicly readable resources. **Regression:** subprocess CLI tests against a loopback server using synthetic credentials: owner exports and destroys a private document, a stranger fails, and public anonymous export still works. Verify private comments are fetched with the same principal as the document.

<a id="r32"></a>

### R32 — P2: S3 signing double-encodes object paths that already contain percent escapes

**Source:** [komodoc/src/s3.rs:58](/home/vincent/repos/komodoc/crates/komodoc/src/s3.rs:58), [komodoc/src/s3.rs:130](/home/vincent/repos/komodoc/crates/komodoc/src/s3.rs:130), [komodoc/src/s3.rs:203](/home/vincent/repos/komodoc/crates/komodoc/src/s3.rs:203).

**Evidence:** high-confidence code inspection; no dedicated end-to-end reproduction.

`url()` applies `escape_path` to the scoped object key. The signer parses that already encoded URL and applies `escape_path` again to `parsed.path()`; presigning repeats the same pattern. A configured prefix containing a space therefore produces `%20` in the outgoing path but `%2520` in the canonical path used for the signature. Default ASCII prefixes conceal the discrepancy.

This is a static finding from the two encoding steps. AWS specifies S3 URI encoding and preservation of object-path identity in its [SigV4 canonical-request documentation](https://docs.aws.amazon.com/AmazonS3/latest/developerguide/sig-v4-header-based-auth.html). No live AWS/R2/MinIO signature rejection was tested, and the repository's local fixture does not independently validate the full signature.

**Correction:** derive the transmitted path and S3 canonical path from one precisely defined raw-key representation, using the S3-specific encoding rules once. **Regression:** validate complete requests and presigned URLs against published vectors plus an independent S3 implementation, including space, `%`, Unicode, and repeated-slash prefixes. Do not generalize the current unreserved-character happy path into a claim of full S3 compatibility.

**Scope note (second review):** `RESERVED` leaves only RFC 3986 unreserved characters unencoded, so the double encoding is triggered by any other character in the scoped key. Every key komodoc builds is made of validated slugs, hex digests, and fixed words, so under current key shapes only the operator-configured prefix can trigger it. Dormant until a prefix contains a space, `%`, or non-ASCII; a startup check that rejects such a prefix would be a cheaper stopgap than reworking the signer.

<a id="r33"></a>

### R33 — P2: The bearer-token cache has no eviction or capacity limit

**Source:** [komodoc/src/auth.rs:798](/home/vincent/repos/komodoc/crates/komodoc/src/auth.rs:798).

**Evidence:** high-confidence code inspection; no dedicated end-to-end reproduction.

`TokenCache::verify` hashes and inserts every distinct nonempty token, including invalid ones. Expiration changes whether an entry can answer a lookup; it does not remove that entry. Tokens that are never requested again remain in the map indefinitely. There is no maximum entry count and no expired-entry sweep.

Repeated requests carrying distinct invalid external bearer tokens can consequently accumulate process memory for the lifetime of the server, while also creating verification traffic on misses. The attacker must reach the external-token verification path; deployment configuration and upstream response time influence the rate. This is a direct lifecycle inspection, not a measured claim about a particular exhaustion rate.

**Correction:** use a bounded cache with actual eviction/expiry cleanup and deduplicate concurrent verification for one token. Apply appropriate admission/rate limits before expensive external verification. **Regression:** many distinct negative tokens over several expiry windows must keep the cache below its capacity and must actually discard expired entries; concurrent identical requests should not create an upstream stampede.

<a id="r34"></a>

### R34 — P2: Removing a slow peer from the room does not close its WebSocket

**Source:** [komodoc/src/room.rs:2361](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:2361), [komodoc/src/server.rs:1038](/home/vincent/repos/komodoc/crates/komodoc/src/server.rs:1038), [komodoc/src/room.rs:1380](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:1380).

**Evidence:** high-confidence code inspection; no dedicated end-to-end reproduction.

Broadcast and persistence paths remove peers from `state.sockets` when `try_send` fails, with comments assuming the socket drops and reconnects. However, `run_socket` retains its own sender clone while its reader task continues, and its writer owns the receiver. Removing the map entry alone therefore need not close the channel or the network connection.

After the queue drains, the writer can wait forever while the still-connected reader is no longer registered for broadcasts. Incoming source updates are ignored because `receive_update` cannot find that socket ID. This produces a connected but detached editor rather than the intended reconnect-and-catch-up behavior. No full network backpressure stress probe was run; the finding is based on channel ownership and the existing removal paths.

**Correction:** give each connection a cancellation mechanism that can terminate both reader and writer even when its outbound queue is full. Remove membership and close the transport together; do not rely on dropping one sender clone. **Regression:** saturate a peer queue, resume reading, and verify an actual close/reconnect followed by complete state catch-up, not merely a reduced room peer count.

<a id="r35"></a>

### R35 — P2: Loading one cold room holds the global room-map lock across storage I/O

**Source:** [komodoc/src/room.rs:435](/home/vincent/repos/komodoc/crates/komodoc/src/room.rs:435), [komodoc/src/blob.rs:204](/home/vincent/repos/komodoc/crates/komodoc/src/blob.rs:204).

**Evidence:** high-confidence code inspection; no dedicated end-to-end reproduction.

`RoomSet::get` takes the global `rooms` mutex and holds it through lease acquisition, room loading, session/history reads, and asset/rendering listings. A cache hit for an unrelated, already loaded room must wait behind all that I/O. Idle eviction under the same lock can add more awaited work. A slow object-store request for one document therefore delays otherwise independent documents.

The filesystem backend further implements prefix listing by scanning the storage tree and filtering results, so cold-room listing cost grows with the whole deployment rather than just that room. Its synchronous filesystem work also runs inside async methods. These conclusions come from lock scope and implementation inspection; production latency/throughput was not benchmarked.

**Correction:** use a short map lock to install or obtain a per-slug loading future/slot, then perform storage I/O outside the global lock. Coordinate duplicate loads of the same slug without serializing all slugs. Narrow filesystem traversal to the requested prefix and move blocking filesystem work off executor workers. **Regression:** pause a cold room's storage GET and assert that a warm room and another independent cold room can still make progress.

## Verification and diagnostics

| Check | Result | Evidence / limit |
|---|---|---|
| `cargo test --workspace --locked` | PASS | 299 binary tests, 25 engine tests, 33 text tests; 357 total; no ignored tests reported |
| `cargo fmt --all -- --check` | PASS | Baseline formatting, before concurrent changes |
| `cargo clippy --workspace --all-targets --all-features --locked` | PASS | Baseline lint run; no Clippy findings |
| Markdown-only WASM `cargo check` | PASS | `--no-default-features --features markdown --target wasm32-unknown-unknown` |
| Typst-only WASM `cargo check` | PASS | `--no-default-features --features typst --target wasm32-unknown-unknown` |
| Focused real-Yjs interoperability rerun | PASS | 15 tests, `--nocapture`; Bun/Node and `web/node_modules/yjs` available; no skip messages |
| Original temporary diagnostics | REPRODUCED | 29 tests passed while asserting observed behavior; R24 is now classified as a policy decision |
| R26 follow-up diagnostic | REPRODUCED | One additional targeted test passed, confirming next-checkpoint deferral after a quiet tick |
| Independent rerun on the current head | REPRODUCED | Second reviewer registered the 29-probe module unchanged in the real tree at `5a29aef` (`cargo test -p komodoc --locked review_probes -- --test-threads=1`): 29 passed, 0 failed. A separately written R26 deferral probe also passed. The module was removed afterwards. R03, R31, R32, R33, R34, R35 re-read in the cited code and confirmed. |

The baseline build warned that the embedded browser Typst renderer was absent (`make typst` would add it). Native Typst tests and the separate Typst WASM compile check succeeded. A successful `cargo check` verifies compilation, not browser loading or visual output, and does not create the production embedded renderer artifact.

Review-time logs are available locally; `/tmp` is temporary, so the durable reproduction source is embedded below:

- `/tmp/komodoc-review-tests.log`
- `/tmp/komodoc-review-fmt.log`
- `/tmp/komodoc-review-clippy.log`
- `/tmp/komodoc-review-wasm-markdown.log`
- `/tmp/komodoc-review-wasm-typst.log`
- `/tmp/komodoc-review-yjs.log`
- `/tmp/komodoc-review-probes.log`
- `/tmp/komodoc-review-r26-followup.log`

Selected actual observations from the original 29-probe run (the separate R26 follow-up is recorded above):

```text
revoked editor: REST 404, existing websocket still reads and writes private document
checkpoint race: live C, persisted B, dirty=false, persist() skips saving C
failed manifest write: retry returns success, checkpoint exists only in memory
successful concurrent label disappeared from memory and persisted manifest
creation returned 201 during checkpoint failure; fresh server reads an empty document
201 after oversized figure: figure and subsequent chapter both absent
directory republish returned 201 but old chapter remained and new file vanished
reconnect erased REMOTE although local file was never edited
emoji replacement 😀 -> 😁 panics in yrs::Text::remove_range
retained history tree read failed: its sole referenced asset was permanently deleted
200000 stored asset/rendering bytes dropped from quota; recorded 114
history_max=1: one checkpoint retained, all 4 unique text blobs retained
200 KB metadata value admitted through 1 KB document ceiling
file limit 5: fresh batch of 20 accepted; no-op on 3 texts refused
test result: ok. 29 passed; 0 failed; 0 ignored
```

### What the existing tests cover well

The test suite has meaningful integration coverage, not just isolated helpers: ownership/sharing, visitor/device flows, CRDT interoperability with actual Yjs, retention, history, renderings, assets, and a local S3-shaped server. Positive characteristics of the implementation include explicit roles, private-resource 404 behavior, separate document-origin handling, storage compare-and-set primitives, deterministic tree serialization, and durable acknowledgments that are ordinarily sent only after a successful session write. The renderer is shared between native and WASM callers; the text crate explicitly uses the UTF-16 coordinate system required by browser Yjs.

No actionable defect was established in `crates/text` itself. Its 33 tests passed. The reproduced reconnect and emoji failures occur in the callers' state/boundary handling, not in `komodoc-text::merge` or its word-level diff. The bounded diff fallback and targeted conflict cases are useful safeguards. This is not a proof over all merge inputs.

### Gaps that explain the green baseline

1. Authorization tests mostly establish access at request/connection creation. They need to carry the same live socket through revocation, expiry, and transfer, and exercise multiple HTTP instances sharing an index.
2. Persistence tests need a systematic fault matrix covering each write and read in a checkpoint, followed by both same-process retry and fresh-process recovery. Successful mutation responses need durable reload assertions.
3. Concurrency tests need controllable storage barriers around snapshot capture, quota reservation, manifest mutation, and cleanup. Single-object compare-and-set tests cannot establish a correct multi-object transaction.
4. Directory tests need full round trips of changed/added/deleted secondary files, invalid middle entries, and complete owner/physical-byte accounting. A valid main file is not evidence of a complete directory upload.
5. Unicode tests need replacements where two emoji share a surrogate, rather than only offset checks around emoji. CRDT admission tests need arbitrary root/value payloads and batched file creation.
6. The S3 fixture recognizes request shape but does not independently verify full SigV4 canonicalization. Cross-check complete requests/presigned URLs with an independent implementation and run a real object-store compatibility suite.
7. Backpressure tests should observe the transport close and client reconnect. Removing an entry from a room map does not demonstrate either.
8. Keep the real-Yjs tests mandatory in an integration CI job: their helper can otherwise return successfully when the JavaScript dependency/runner is absent. This review verified availability, but a generic green Rust run elsewhere may omit that work.

### Further concerns and design questions

These are not counted as additional confirmed findings. They identify follow-up work and relevant limits on the review:

- **Physical quota model:** `Manifest::bytes()` sums checkpoint logical sizes, and tree sizes include referenced files/assets. Shared blobs can be charged repeatedly even though storage deduplicates them; R20/R21 demonstrate undercharging on other paths. Establish one explicit physical-object model before adjusting isolated arithmetic. Also distinguish an unlimited allowance from a *negative remaining* allowance: `allowance()` subtracts session bytes, while checkpoint shedding treats any negative ceiling as unlimited.
- **Deletion during activity:** inspect purge/delete with live `Arc<Room>` holders and a simultaneous sweep/checkpoint. Removing the map entry does not make outstanding references inert. Storage deletion failures and index-write failure need a recoverable tombstone/transaction story. This review did not run a full delete-versus-checkpoint fault matrix.
- **Filesystem multi-process semantics:** the filesystem backend's mutex belongs to one store instance; it is not an operating-system lock across processes. Clarify whether that backend supports multiple processes sharing a directory, and enforce/document the answer. Do not infer inter-process atomic compare-and-set from an in-process mutex test.
- **Sync settle/lock behavior:** review disk writes arriving during the debounce window, and the lock file's check-then-create sequence. A common-base fix for R13 should be accompanied by end-to-end filesystem event tests, including two clients starting together.
- **Native nested-main roots:** directory publishing preflights the main file through a file-oriented renderer. Confirm that `chapters/main.typ` may read other files inside the chosen publication directory using the same root semantics as the browser. This was inspected but not separately reproduced.
- **Single-file LaTeX naming:** `main_path_for` does not select `main.tex` for the LaTeX format. Decide whether extension-based renderer selection or the explicit format is authoritative and make both consistent (related to R27).
- **Credential-at-rest creation:** `write_token` writes the file and then chmods it, ignoring a permission-setting failure. Create private credentials with restrictive permissions from the first open and propagate permission errors; also consider symlink handling. No actual permission exposure was measured.
- **Operational boundaries:** no live OAuth providers, SMTP service, production bucket, Windows/macOS runtime, browser UI automation, dependency-vulnerability audit, long-running load test, or exhaustive adversarial fuzz campaign was performed. WASM checks cover compilation only. No claim about those environments should be inferred from the native tests.

Recommended implementation order, treating cost bounding as the stated priority:

1. Fix R26 immediately to stop recurring idle work and unintended checkpoint deferral. R30 is another small, well-contained correction that can be completed alongside it.
2. Address R21 and R20 together under one physical-object accounting model. Include R18, R19, and R22 in the same cost-bounding milestone: correcting recorded totals does not close admission holes.
3. Repair checkpoint transaction/state handling for R07, R08, and R09, with failure recovery and generation-aware completion. Follow with R15/R16 where unreadable state currently becomes data loss.
4. Fix R13, R12, and the successful-but-incomplete publication paths R10/R11.
5. Address R01/R02, R03/R31 together for CLI credential handling, and R05. In a currently exposed private multi-user deployment, revocation and accidental disclosure fixes belong in the first milestone rather than waiting for the entire cost project.
6. Fix R25 and the remaining P2 compatibility/operational defects. Decide R24’s consistency contract with an explicit failure-model test.

R07 and R09 involve concurrent stale state, but R08 also reproduces with one caller: the failed storage write leaves memory marked committed and defeats retry. A larger lock alone cannot fix it. A shared persistence transaction design is appropriate; it must include committed-versus-staged state and error recovery, not merely hold the edit lock across more network I/O.

Likewise, R19 has two errors: using the old count in fast admission and counting text files twice in the exact path. It should be a small fix, but correcting a single comparison is not sufficient to repair both invariants.

### Reproducing the diagnostics

The temporary workspace used for the review is `/tmp/komodoc-review-work`. Its only Rust edits were the following module and `mod review_probes;` added to `crates/komodoc/src/tests/mod.rs`. No review probes or production fixes were added to the real `crates/` tree.

To recreate later, make a disposable checkout/copy of the stated commit, install the repository's ordinary build dependencies, save the module below as `crates/komodoc/src/tests/review_probes.rs`, and register it in `tests/mod.rs`. The module also compiles and passes unchanged when registered directly in this working tree at `5a29aef`, which is the quickest way to rerun it; remove the file and the `mod review_probes;` line afterwards. `review_relative_directory_gitignore_is_not_honored` creates and removes a `komodoc-gitignore-*` directory under the process's working directory. The existing test harness and private crate modules are intentionally reused. Run on Linux/Unix because the symlink probe uses `std::os::unix`. The working tree must be writable for the disposable relative-Git-root test. The full repository's `web`, `examples`, and documentation resources are needed by the existing build/tests; the review copy linked those to this checkout.

Actual review command:

```sh
cargo test -p komodoc --locked   --manifest-path /tmp/komodoc-review-work/Cargo.toml   --target-dir /home/vincent/repos/komodoc/target   review_probes -- --nocapture --test-threads=1
```

The tests below assert current observed behavior so that a successful run is a reproducible diagnostic. Convert confirmed bug assertions into the desired invariants when adding permanent regression tests. Do not mechanically invert the lease or external-symlink assertions before settling those policies. In particular, the emoji probe intentionally catches an unwind; its printed panic is expected diagnostic output. It is not a recommended production error-handling strategy.

<details>
<summary>Complete diagnostic module (30 probes, including the R26 follow-up)</summary>

```rust
//! Temporary review diagnostics: assertions document observed defects.
use super::*;
use crate::{session, room, blob, history, store};
use crate::blob::{BlobStore, BlobResult, BlobVersion, BlobInfo, BlobError};
use crate::config::Configuration;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use serde_json::json;
use yrs::{Map, Transact};

async fn fixture(config: Configuration) -> (tempfile::TempDir, Arc<store::Store>, room::RoomSet) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path()));
    let config = Arc::new(config);
    let store = Arc::new(store::Store::open(blobs.clone(), config.clone()).await.unwrap());
    let rooms = room::RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    store.put(store::Publication {slug:"probe".into(), source:"A".into(), source_format:"markdown".into(), owner:"alice".into(), ..Default::default()}).await.unwrap();
    let room = rooms.get("probe").await;
    room.set_source("A", "markdown").await;
    room.checkpoint("comment", "alice").await.unwrap();
    (dir, store, rooms)
}

#[test]
fn review_file_count_fast_path_and_double_count() {
    let doc = session::new_doc();
    for i in 0..20 { session::put_text(&doc, &format!("{i}.txt"), "x"); }
    let empty = session::new_doc();
    let update = session::encode_state(&doc);
    assert_eq!(session::admit_update(&empty, &update, 100000, 5), session::Admission::Fits);
    let legitimate = session::new_doc();
    for i in 0..3 { session::put_text(&legitimate, &format!("{i}.txt"), "x"); }
    assert_eq!(session::admit_update(&legitimate, &[0,0], 100000, 5), session::Admission::TooMany);
    println!("file limit 5: fresh batch of 20 accepted; no-op on 3 texts refused");
}

#[test]
fn review_metadata_bypasses_byte_limit() {
    let doc = session::new_doc();
    let meta = doc.get_or_insert_map(session::META);
    meta.insert(&mut doc.transact_mut(), "payload", "x".repeat(200000));
    let empty = session::new_doc();
    let update = session::encode_state(&doc);
    assert!(update.len() > 200000);
    assert_eq!(session::admit_update(&empty, &update, 1024, 200), session::Admission::Fits);
    println!("200 KB metadata value admitted through 1 KB document ceiling");
}

#[tokio::test]
async fn review_sync_reconnect_rolls_back_remote() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    std::fs::write(&path,"alpha beta").unwrap();
    let mut client = crate::sync::Client::new(path,Duration::from_millis(50),String::new(),String::new());
    let remote = session::new_doc();
    session::replace_text(&remote,"alpha beta","main.md");
    let state = || json!({"type":"y-state","update":room::encode_update(&session::encode_state(&remote))}).to_string();
    client.receive(&state()).await.unwrap();
    client.take_outbox();
    session::replace_text(&remote,"alpha REMOTE beta","main.md");
    client.receive(&state()).await.unwrap();
    assert_eq!(session::text_of(client.document()),"alpha beta");
    println!("reconnect erased REMOTE although local file was never edited");
}

#[tokio::test]
async fn review_revisiting_checkpoint_leaves_wrong_head() {
    let (_dir,store,rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let a = room.tree().await.digest();
    room.set_source("B","markdown").await;
    let b = room.checkpoint("comment","alice").await.unwrap().unwrap();
    room.set_source("A","markdown").await;
    assert_eq!(room.checkpoint("comment","alice").await.unwrap().unwrap(),a);
    assert_eq!(store.get("probe").await.unwrap().sha,b);
    assert_eq!(room.manifest().await.latest().unwrap().sha,b);
    println!("A -> B -> A: returned A but index and latest history still name B");
}

#[tokio::test]
async fn review_persist_forgets_asset_and_rendering_charges() {
    let (_dir,store,rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    room.put_asset(vec![1;100000],(200000,200000)).await.unwrap();
    let sha = room.tree().await.digest();
    room.put_rendering(&sha,false,vec![2;100000]).await.unwrap();
    assert!(store.get("probe").await.unwrap().size >= 200000);
    room.set_source("A changed","markdown").await;
    room.persist().await.unwrap();
    let charge = store.get("probe").await.unwrap().size;
    assert!(charge < 1000);
    assert_eq!(room.assets_bytes().await + room.renderings_bytes().await,200000);
    println!("200000 stored asset/rendering bytes dropped from quota; recorded {charge}");
}

#[tokio::test]
async fn review_shed_history_leaks_text_blobs() {
    let mut config=Configuration::default(); config.session.history_max=1;
    let (_dir,store,rooms)=fixture(config).await;
    let room=rooms.get("probe").await;
    for s in ["B","C","D"] {room.set_source(s,"markdown").await; room.checkpoint("comment","").await.unwrap();}
    assert_eq!(room.manifest().await.checkpoints.len(),1);
    let objects=store.blobs.list("history/probe/blobs/").await.unwrap();
    assert_eq!(objects.len(),4);
    println!("history_max=1: one checkpoint retained, all 4 unique text blobs retained");
}

#[tokio::test]
async fn review_quiet_room_never_reports_idle() {
    let (_dir,_store,rooms)=fixture(Configuration::default()).await;
    let room=rooms.get("probe").await;
    {let mut state=room.state.lock().await; state.session.updated_at=0; state.touched=0;}
    assert!(!room.tick().await);
    assert!(!room.tick().await);
    println!("clean room with no peers never reports idle: text digest compared with tree digest");
}

#[tokio::test]
async fn review_changing_main_does_not_change_format() {
    let (_dir,store,rooms)=fixture(Configuration::default()).await;
    let room=rooms.get("probe").await;
    {let state=room.state.lock().await; let id=session::put_text(&state.session.doc,"paper.typ","= Typst"); session::set_main(&state.session.doc,&id);}
    room.checkpoint("comment","").await.unwrap();
    let entry=store.get("probe").await.unwrap();
    assert_eq!(entry.main,"paper.typ"); assert_eq!(entry.source_format,"markdown");
    println!("main paper.typ is still advertised and checkpointed as markdown");
}

async fn directory(server: &TestServer, slug:&str, extra:Vec<(&str,Vec<u8>)>) -> (u16,serde_json::Value) {
    let mut form=reqwest::multipart::Form::new().text("title","Directory").text("main","main.md").text("slug",slug.to_string())
      .part("file",reqwest::multipart::Part::bytes(b"# Main".to_vec()).file_name("main.md"));
    for (path,bytes) in extra { form=form.part("file",reqwest::multipart::Part::bytes(bytes).file_name(path.to_string())); }
    let response=client().post(format!("{}/api/documents",server.url)).header("cookie",session_as(TEST_PUBLISHER)).header("x-komodoc-client","1").multipart(form).send().await.unwrap();
    (response.status().as_u16(),response.json().await.unwrap())
}

#[tokio::test]
async fn review_directory_republish_ignores_chapters() {
    let server=new_test_server().await;
    let (status,entry)=directory(&server,"",vec![("chapter.txt",b"old".to_vec())]).await; assert_eq!(status,201);
    let slug=text(&entry,"slug");
    let (status,_)=directory(&server,&slug,vec![("chapter.txt",b"NEW".to_vec()),("new.txt",b"ADDED".to_vec())]).await; assert_eq!(status,201);
    let room=server.instance.rooms.get(&slug).await;
    let state=room.state.lock().await;
    let texts=session::texts_of(&state.session.doc);
    assert_eq!(texts["chapter.txt"],"old"); assert!(!texts.contains_key("new.txt"));
    println!("directory republish returned 201 but old chapter remained and new file vanished");
}

#[tokio::test]
async fn review_directory_accepts_partial_failure() {
    let mut config=Configuration::default(); config.max_asset=10;
    let server=test_server_with(config,crate::auth::Policy::parse(TEST_PUBLISHER),crate::auth::Policy::parse("anyone"),true).await;
    let (status,entry)=directory(&server,"",vec![("big.png",vec![1;11]),("chapter.txt",b"missing".to_vec())]).await;
    assert_eq!(status,201);
    let tree=server.instance.rooms.get(&text(&entry,"slug")).await.tree().await;
    assert_eq!(tree.files.len(),1);
    println!("201 after oversized figure: figure and subsequent chapter both absent");
}

#[tokio::test]
async fn review_directory_body_limit_ignores_figure_allowance() {
    let server=new_test_server().await;
    let (status,_)=directory(&server,"",vec![("plot.png",vec![1;3*1024*1024])]).await;
    assert_eq!(status,400);
    println!("3 MiB figure (max_asset=8 MiB) rejected with 400 by implicit 2 MiB multipart limit");
}

#[tokio::test]
async fn review_revoked_socket_can_read_and_write() {
    let server=test_server_with(Configuration::default(),crate::auth::Policy::parse("any"),crate::auth::Policy::parse("anyone"),true).await;
    let (status,entry)=post_as(&session_as("alice"),&server.url,"/api/documents",json!({"title":"Private","source":"secret","source_format":"markdown"})).await; assert_eq!(status,201);
    let slug=text(&entry,"slug"); let share=format!("/api/documents/{slug}/share");
    assert_eq!(post_as(&session_as("alice"),&server.url,&share,json!({"visibility":"private","grant":{"login":"bob","role":"editor"}})).await.0,200);
    let mut socket=dial_websocket_with(&server.url,&slug,&format!("Cookie: {}\r\n",session_as("bob"))).await.unwrap();
    assert_eq!(socket.read().await["type"],"hello");
    assert_eq!(post_as(&session_as("alice"),&server.url,&share,json!({"revoke":"bob"})).await.0,200);
    assert_eq!(get_json_as(&session_as("bob"),&server.url,&format!("/api/documents/{slug}")).await.0,404);
    socket.write(json!({"type":"y-open"})).await;
    let state=socket.read().await; assert_eq!(state["type"],"y-state");
    let doc=session::new_doc(); session::apply_update(&doc,&room::decode_update(&text(&state,"update")).unwrap()).unwrap();
    let before=session::encode_vector(&doc); session::replace_text(&doc,"REVOKED WRITER","main.md");
    socket.write(json!({"type":"y-update","update":room::encode_update(&session::encode_diff(&doc,&before).unwrap()),"seq":1})).await;
    tokio::time::timeout(Duration::from_secs(2),async { loop {if server.instance.rooms.get(&slug).await.source().await=="REVOKED WRITER" {break;} tokio::task::yield_now().await;} }).await.unwrap();
    println!("revoked editor: REST 404, existing websocket still reads and writes private document");
}

struct HookStore {inner:Arc<dyn BlobStore>, pause:Mutex<Option<(String,String)>>, fail:Mutex<Option<String>>, reached:tokio::sync::Notify, resume:tokio::sync::Notify}
impl HookStore {
    fn new(inner:Arc<dyn BlobStore>)->Arc<Self>{Arc::new(Self{inner,pause:Mutex::new(None),fail:Mutex::new(None),reached:tokio::sync::Notify::new(),resume:tokio::sync::Notify::new()})}
    async fn hook(&self,op:&str,key:&str)->BlobResult<()> {
        let pause={let mut p=self.pause.lock().unwrap();if p.as_ref().is_some_and(|(o,k)|o==op&&k==key){p.take();true}else{false}};
        if pause {self.reached.notify_one();self.resume.notified().await;}
        if self.fail.lock().unwrap().as_ref().is_some_and(|f|key.starts_with(f)){return Err(BlobError::Other("injected review failure".into()));} Ok(())
    }
}
#[async_trait::async_trait]
impl BlobStore for HookStore {
 async fn get(&self,k:&str)->BlobResult<Vec<u8>>{self.hook("get",k).await?;self.inner.get(k).await}
 async fn get_versioned(&self,k:&str)->BlobResult<(Vec<u8>,BlobVersion)>{self.hook("get_versioned",k).await?;self.inner.get_versioned(k).await}
 async fn put(&self,k:&str,b:Vec<u8>,t:&str)->BlobResult<()>{self.hook("put",k).await?;self.inner.put(k,b,t).await}
 async fn swap(&self,k:&str,b:Vec<u8>,e:&str)->BlobResult<BlobVersion>{self.hook("swap",k).await?;self.inner.swap(k,b,e).await}
 async fn list(&self,p:&str)->BlobResult<Vec<BlobInfo>>{self.inner.list(p).await}
 async fn delete(&self,k:&[String])->BlobResult<()>{self.inner.delete(k).await}
 fn describe(&self)->String{self.inner.describe()}
}

#[tokio::test]
async fn review_checkpoint_race_drops_dirty_edit() {
    let (_dir,store,rooms)=fixture(Configuration::default()).await;
    let old=rooms.get("probe").await; let sha=old.tree().await.digest();
    store.blobs.delete(&[blob::room_lock_key("probe")]).await.unwrap();
    let hooked=HookStore::new(store.blobs.clone());
    let reopened=room::RoomSet::new(hooked.clone(),Arc::new(Configuration::default())); reopened.attach_store(store.clone());
    let room=reopened.get("probe").await;
    room.set_source("B","markdown").await;
    *hooked.pause.lock().unwrap()=Some(("get".into(),blob::checkpoint_key("probe",&sha)));
    let task=tokio::spawn({let room=room.clone();async move{room.checkpoint("comment","").await}});
    tokio::time::timeout(Duration::from_secs(2),hooked.reached.notified()).await.unwrap();
    room.set_source("C AFTER SNAPSHOT","markdown").await;
    hooked.resume.notify_one(); task.await.unwrap().unwrap();
    assert_eq!(room.source().await,"C AFTER SNAPSHOT");
    assert!(!room.state.lock().await.session.dirty);
    assert!(!room.persist().await.unwrap());
    let saved=session::new_doc(); session::apply_update(&saved,&store.blobs.get(&blob::session_key("probe")).await.unwrap()).unwrap();
    assert_eq!(session::text_of(&saved),"B");
    println!("checkpoint race: live C, persisted B, dirty=false, persist() skips saving C");
}

#[tokio::test]
async fn review_failed_manifest_write_never_retries() {
    let (_dir,store,_rooms)=fixture(Configuration::default()).await;
    store.blobs.delete(&[blob::room_lock_key("probe")]).await.unwrap();
    let hooked=HookStore::new(store.blobs.clone()); let rooms=room::RoomSet::new(hooked.clone(),Arc::new(Configuration::default()));rooms.attach_store(store.clone());
    let room=rooms.get("probe").await; room.set_source("B","markdown").await;
    *hooked.fail.lock().unwrap()=Some(blob::history_index_key("probe"));
    assert!(room.checkpoint("comment","").await.is_err());
    *hooked.fail.lock().unwrap()=None;
    let b=room.checkpoint("comment","").await.unwrap().unwrap();
    let disk=history::load(store.blobs.as_ref(),"probe").await.unwrap();
    assert!(!disk.has(&b)); assert!(room.manifest().await.has(&b));
    println!("failed manifest write: retry returns success, checkpoint exists only in memory");
}

#[tokio::test]
async fn review_index_conflict_stays_stale() {
    let (_dir,store,_rooms)=fixture(Configuration::default()).await;
    let second=store::Store::open(store.blobs.clone(),Arc::new(Configuration::default())).await.unwrap();
    store.modify("probe",|e|{e.visibility="private".into();Ok(())}).await.unwrap();
    for _ in 0..2 {assert!(second.modify("probe",|e|{e.title="x".into();Ok(())}).await.is_err());}
    assert_ne!(second.get("probe").await.unwrap().visibility,"private");
    println!("second Store keeps stale public visibility and cannot modify after index CAS conflict");
}

#[tokio::test]
async fn review_lease_error_reports_held() {
    let dir=tempfile::tempdir().unwrap(); let hooked=HookStore::new(Arc::new(blob::FsStore::new(dir.path())));
    *hooked.fail.lock().unwrap()=Some(blob::room_lock_key("probe"));
    assert!(blob::take_room_lease(hooked.as_ref(),"probe","server-b",Some(1)).await.held);
    println!("lease read failure returns held=true with a fresh timestamp");
}

#[tokio::test]
async fn review_typst_reader_follows_external_symlink() {
    let dir=tempfile::tempdir().unwrap(); let outside=tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("private.txt"),"PRIVATE OUTSIDE ROOT").unwrap();
    std::os::unix::fs::symlink(outside.path(),dir.path().join("linked")).unwrap();
    let compiled=crate::render::render_typst_document(&dir.path().join("main.typ"),"#read(\"linked/private.txt\")","");
    assert!(compiled.page.unwrap().contains("PRIVATE OUTSIDE ROOT"));
    assert!(crate::cli::files_under(dir.path(),"",&|_|false).contains(&"linked/private.txt".to_string()));
    println!("Typst renderer and directory publisher both follow symlink outside root");
}

#[test]
fn review_replacing_emoji_splits_surrogate_pair() {
    let doc=session::new_doc(); session::replace_text(&doc,"😀","main.md");
    let outcome=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session::replace_text(&doc,"😁","main.md")));
    assert!(outcome.is_err());
    println!("emoji replacement 😀 -> 😁 panics in yrs::Text::remove_range");
}

#[tokio::test]
async fn review_concurrent_asset_admission_exceeds_limit() {
    let (_dir,store,_rooms)=fixture(Configuration::default()).await;
    store.blobs.delete(&[blob::room_lock_key("probe")]).await.unwrap();
    let hooked=HookStore::new(store.blobs.clone());let rooms=room::RoomSet::new(hooked.clone(),Arc::new(Configuration::default())); rooms.attach_store(store);
    let room=rooms.get("probe").await;
    let key=blob::asset_key("probe",&store::digest_of_bytes(&[1;10]));
    *hooked.pause.lock().unwrap()=Some(("put".into(),key));
    let task=tokio::spawn({let room=room.clone();async move{room.put_asset(vec![1;10],(15,15)).await}});
    tokio::time::timeout(Duration::from_secs(2),hooked.reached.notified()).await.unwrap();
    room.put_asset(vec![2;10],(15,15)).await.unwrap();
    hooked.resume.notify_one();task.await.unwrap().unwrap();
    assert_eq!(room.assets_bytes().await,20);
    println!("two concurrent 10-byte assets accepted under 15-byte aggregate ceiling");
}

#[tokio::test]
async fn review_concurrent_label_is_lost_by_checkpoint() {
    let (_dir,store,_rooms)=fixture(Configuration::default()).await;
    let a=store.get("probe").await.unwrap().sha;
    store.blobs.delete(&[blob::room_lock_key("probe")]).await.unwrap();
    let hooked=HookStore::new(store.blobs.clone());let hooked_store=Arc::new(store::Store::open(hooked.clone(),Arc::new(Configuration::default())).await.unwrap());let rooms=room::RoomSet::new(hooked.clone(),Arc::new(Configuration::default()));rooms.attach_store(hooked_store);
    let room=rooms.get("probe").await;room.set_source("B","markdown").await;
    *hooked.pause.lock().unwrap()=Some(("swap".into(),blob::INDEX_KEY.into()));
    let task=tokio::spawn({let room=room.clone();async move{room.checkpoint("comment","").await}});
    tokio::time::timeout(Duration::from_secs(2),hooked.reached.notified()).await.unwrap();
    assert!(room.label(&a,"accepted label").await.unwrap());
    hooked.resume.notify_one();task.await.unwrap().unwrap();
    assert!(room.manifest().await.checkpoints.iter().find(|p|p.sha==a).unwrap().label.is_empty());
    assert!(history::load(store.blobs.as_ref(),"probe").await.unwrap().checkpoints.iter().find(|p|p.sha==a).unwrap().label.is_empty());
    println!("successful concurrent label disappeared from memory and persisted manifest");
}

#[tokio::test]
async fn review_creation_reports_success_without_saved_source() {
    let dir=tempfile::tempdir().unwrap();let inner:Arc<dyn BlobStore>=Arc::new(blob::FsStore::new(dir.path()));let hooked=HookStore::new(inner.clone());
    let (url,server)=server_over_blobs(hooked.clone(),Configuration::default()).await;
    *hooked.fail.lock().unwrap()=Some("history/".into());
    let (status,entry)=post(&url,"/api/documents",json!({"title":"Unsaved","source":"ONLY IN RAM","source_format":"markdown"})).await;
    assert_eq!(status,201);let slug=text(&entry,"slug");
    assert_eq!(server.rooms.get(&slug).await.source().await,"ONLY IN RAM");
    assert!(inner.get(&blob::session_key(&slug)).await.is_err());
    *hooked.fail.lock().unwrap()=None;
    let (_url,restarted)=server_over_blobs(inner,Configuration::default()).await;
    assert_eq!(restarted.rooms.get(&slug).await.source().await,"");
    println!("creation returned 201 during checkpoint failure; fresh server reads an empty document");
}

#[tokio::test]
async fn review_corrupt_session_overwritten_on_load() {
    let (_dir,store,_rooms)=fixture(Configuration::default()).await;
    store.blobs.put(&blob::session_key("probe"),vec![255],"application/octet-stream").await.unwrap();
    store.blobs.delete(&[blob::room_lock_key("probe")]).await.unwrap();
    let rooms=room::RoomSet::new(store.blobs.clone(),Arc::new(Configuration::default()));rooms.attach_store(store.clone());
    let room=rooms.get("probe").await;
    assert!(!room.read_only());assert_eq!(room.source().await,"");
    room.set_source("NEW EMPTY-BASE CONTENT","markdown").await;
    room.persist().await.unwrap();
    let doc=session::new_doc();session::apply_update(&doc,&store.blobs.get(&blob::session_key("probe")).await.unwrap()).unwrap();
    assert_eq!(session::text_of(&doc),"NEW EMPTY-BASE CONTENT");
    println!("corrupt session loaded writable and was replaced; valid older checkpoint not used");
}

#[tokio::test]
async fn review_session_key_transient_read_rotates_key() {
    let dir=tempfile::tempdir().unwrap();let inner:Arc<dyn BlobStore>=Arc::new(blob::FsStore::new(dir.path()));
    let original=crate::auth::session_key(inner.as_ref()).await.unwrap();
    struct FailRead(Arc<dyn BlobStore>);
    #[async_trait::async_trait] impl BlobStore for FailRead {
     async fn get(&self,_:&str)->BlobResult<Vec<u8>>{Err(BlobError::Other("transient GET failure".into()))}
     async fn get_versioned(&self,k:&str)->BlobResult<(Vec<u8>,BlobVersion)>{self.0.get_versioned(k).await}
     async fn put(&self,k:&str,b:Vec<u8>,t:&str)->BlobResult<()>{self.0.put(k,b,t).await}
     async fn swap(&self,k:&str,b:Vec<u8>,e:&str)->BlobResult<BlobVersion>{self.0.swap(k,b,e).await}
     async fn list(&self,k:&str)->BlobResult<Vec<BlobInfo>>{self.0.list(k).await}
     async fn delete(&self,k:&[String])->BlobResult<()>{self.0.delete(k).await}
     fn describe(&self)->String{self.0.describe()}
    }
    let rotated=crate::auth::session_key(&FailRead(inner.clone())).await.unwrap();assert_ne!(rotated,original);
    assert_eq!(crate::auth::session_key(inner.as_ref()).await.unwrap(),rotated);
    println!("temporary session.key read failure silently replaced existing signing key");
}

#[tokio::test]
async fn review_mirror_double_encoded_parent_escapes_base() {
    let router=axum::Router::new().fallback(|uri:axum::http::Uri|async move{uri.path().to_string()});
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
    tokio::spawn(async move{axum::serve(listener,router).await.unwrap()});
    let mirror=crate::latex::Mirror::Upstream{base:format!("http://{addr}/latex/"),client:reqwest::Client::new()};
    let result=mirror.get("%252e%252e/private.json").await;
    assert_eq!(String::from_utf8(result.bytes).unwrap(),"/private.json");
    println!("/latex/%252e%252e/private.json proxied upstream /private.json outside /latex/");
}

#[test]
fn review_relative_directory_gitignore_is_not_honored() {
    let dir=tempfile::Builder::new().prefix("komodoc-gitignore-").tempdir_in(".").unwrap();
    assert!(std::process::Command::new("git").args(["init","-q"]).arg(dir.path()).status().unwrap().success());
    std::fs::write(dir.path().join(".gitignore"),"/private.txt\n").unwrap();
    std::fs::write(dir.path().join("private.txt"),"DO NOT PUBLISH").unwrap();
    let relative=dir.path().strip_prefix(std::env::current_dir().unwrap()).unwrap_or(dir.path());
    let found=crate::cli::files_under(relative,"",&crate::cli::git_ignores(relative));
    assert!(found.contains(&"private.txt".to_string()));
    println!("publishing a relative directory includes private.txt despite its .gitignore entry");
}

#[tokio::test]
async fn review_failed_tree_read_prunes_retained_asset() {
    let mut config=Configuration::default();config.asset_grace=0;
    let (_dir,store,rooms)=fixture(config.clone()).await;
    let room=rooms.get("probe").await;
    let (sha,_)=room.put_asset(vec![9;10],(100,100)).await.unwrap();
    room.name_asset("fig.png",&sha).await;
    let old=room.checkpoint("comment","").await.unwrap().unwrap();
    store.blobs.delete(&[blob::room_lock_key("probe")]).await.unwrap();
    let hooked=HookStore::new(store.blobs.clone());let reopened=room::RoomSet::new(hooked.clone(),Arc::new(config));reopened.attach_store(store.clone());
    let room=reopened.get("probe").await;
    {let state=room.state.lock().await;state.session.doc.get_or_insert_map(session::ASSETS).remove(&mut state.session.doc.transact_mut(),"fig.png");}
    room.set_source("B","markdown").await;
    *hooked.fail.lock().unwrap()=Some(blob::checkpoint_key("probe",&old));
    room.checkpoint("comment","").await.unwrap();
    assert!(room.manifest().await.has(&old));
    assert!(store.blobs.get(&blob::asset_key("probe",&sha)).await.is_err());
    println!("retained history tree read failed: its sole referenced asset was permanently deleted");
}

#[test]
fn review_markdown_encoded_image_path_not_resolved() {
    let html=komodoc_engine::markdown::render_with("![plot](<fig/my plot.png>)","",&|path| {
        if path=="fig/my plot.png" {Some("data:image/png;base64,AAAA".into())}else{None}
    });
    assert!(html.contains("src=\"fig/my%20plot.png\""));assert!(!html.contains("data:image/png"));
    println!("Markdown image filename with space is URL-encoded before filesystem asset lookup and stays broken");
}

#[test]
fn review_response_omits_figure_replies() {
    let item=room::Comment {body:"figure question".into(),region:Some(room::Region::default()),replies:vec![room::Reply {body:"THE AUTHOR ANSWER".into(),creator:"Author".into(),..Default::default()}],..Default::default()};
    let report=crate::export::render_response("title",&[item],"",&Configuration::default(),"");
    assert!(report.contains("figure question"));assert!(!report.contains("THE AUTHOR ANSWER"));
    println!("response-to-reviewers export includes figure question and omits its author's answer");
}

#[tokio::test]
async fn review_read_only_room_relays_edits() {
    let (_dir,store,_rooms)=fixture(Configuration::default()).await;
    let other=room::RoomSet::new(store.blobs.clone(),Arc::new(Configuration::default()));other.attach_store(store);
    let room=other.get("probe").await;assert!(room.read_only());
    let (tx,_rx)=tokio::sync::mpsc::channel(10);room.attach(1,"test".into(),tx,true).await;
    let doc=session::new_doc();session::apply_update(&doc,&room.open_state(None).await.0).unwrap();let before=session::encode_vector(&doc);session::replace_text(&doc,"UNSAVABLE EDIT","main.md");
    assert!(matches!(room.receive_update(1,&session::encode_diff(&doc,&before).unwrap(),1,"alice").await,room::Applied::Relay));
    assert_eq!(room.source().await,"UNSAVABLE EDIT");assert!(room.persist().await.is_err());
    println!("room held by another server accepts and relays edits which persist refuses to save");
}

#[tokio::test]
async fn review_idle_tick_defers_next_changed_checkpoint() {
    let (_dir,store,rooms)=fixture(Configuration::default()).await;
    let room=rooms.get("probe").await;
    let old=store.get("probe").await.unwrap().sha;
    {let mut state=room.state.lock().await;state.session.updated_at=0;state.session.last_checkpoint_at=1;state.touched=0;}
    assert!(!room.tick().await);
    room.set_source("B AFTER LONG IDLE","markdown").await;
    assert!(room.checkpoint("cli","alice").await.unwrap().is_none());
    assert_eq!(store.get("probe").await.unwrap().sha,old);
    println!("one no-op idle tick refreshes checkpoint time; next changed cli checkpoint is deferred and index keeps the old SHA");
}
```

</details>


### Follow-up calibration

After feedback from another reviewer, R04 and R14 were lowered to P2, R06 was narrowed to local file-selection policy and traversal robustness at P2, and R24 was removed from the defect count and retained as a consistency-policy decision. R26 gained an independently reproduced consequence and a higher place in the work order. R05 remains P1 despite its conditional trigger; R28 and R32 remain conditional P2 findings. The original findings and source snapshot remain traceable by their stable IDs. Only the additional R26 diagnostic was run in this follow-up; the other reviewer’s full current-HEAD rerun is supplied corroboration, not a second run claimed here.

## Review disposition

Use the confirmed defects as a remediation work list and R24 as a design question. The second review concurs with the findings and the revised priorities; its only remaining reservations are the scope notes on R28 and R32 above, and a view that R26 sits at the top of P2 because it changes what every publish over an idle document reports. This review changed only `REVIEW-codex-crates.md` in the real workspace and left concurrent source edits intact. It did not apply fixes, create commits, or publish results externally. The code-reviewer and Rust-engineer skills were used for the review; no reusable skill correction was necessary.
