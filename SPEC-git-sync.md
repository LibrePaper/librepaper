# Git and file sync, first phase

Status: proposed 2026-10-26

This is a focused first-phase proposal. It replaces the earlier 683-line draft of this document. It supersedes only the named-version export, Git-panel wording, branch pinning, retry-state, and first-link preview portions of earlier Git specs where they conflict. `SPEC-sync.md` remains the design for the existing two-command folder sync. This document does not introduce a combined sync operation.

## 1. Product principle

> LibrePaper transfers current files and explains their destination. Git handles merges.

The user chooses a source and destination, starts a transfer, and gets a truthful result. LibrePaper does not hide branch selection, silently switch branches, or claim that an operation completed when one side did not.

The first phase makes five changes:

1. A web pull imports ordered named versions and then the current accepted project snapshot when it differs. Unnamed accepted edits are no longer missed. A cheap head check keeps a quiet pull from reading the whole project.
2. The UI names each destination and keeps folder-to-web transfer separate from native Git operations.
3. A Git connection binds to one selected branch and refuses to follow a different checkout silently.
4. A web push saves a small pending request in existing sync state before posting, so an ordinary retry can recover the outcome.
5. A pull preview inventories the first-link differences without changing files or binding state.

The existing two-command workflow remains intact:

| Command | Direction | Destination |
|---|---|---|
| `librepaper pull` | Web project to folder or repository | Current working tree, or a Git merge into the selected branch |
| `librepaper push` | Folder or repository to web project | Current accepted project files |

The GitHub Action retains its event-specific behavior: repository pushes send repository changes to the web project; scheduled or manually dispatched runs import web changes into the repository. A run does not promise one atomic, bidirectional exchange.

## 2. Scope

### In scope

- Capture current accepted project files after importing unseen named versions.
- Add a cheap change signal so a scheduled or status poll that finds nothing new makes one indexed read and no project read.
- Preserve existing named-version ordering, attribution, adoption, and cursor behavior.
- Add explicit destination wording to desktop actions and sync status.
- Bind Git sync state to one explicitly selected branch.
- Persist and recover an in-flight web-push request using the existing State store and server replay contract.
- Add a read-only first-link preview and recheck when the user starts the real pull.
- Update the GitHub Action and documentation to describe the current-head behavior and direction of each event.

### Out of scope

- A new bidirectional `librepaper sync` command or a shared desktop/CLI/Action orchestrator.
- A new folder synchronization algorithm, folder baseline database, or three-pane conflict editor.
- New conflict resolution semantics. Existing Git merge and folder whole-file conflict choices remain.
- A server-side connection registry, credential storage for a Git host, or project-global branch uniqueness.
- A durable multi-step distributed transaction, leased journal, or general fresh-runner recovery guarantee.
- A redesign of the Action's conflict-branch or pull-request lifecycle.
- Automatically switching branches, committing local changes, or overriding normal Git protections.
- Syncing comments, suggestions, chat, or other project metadata into Git.

## 3. Current behavior and constraints

These are implementation facts, not proposed behavior.

### 3.1 File transfer

- `docs/git.md` documents `librepaper pull` and `librepaper push` as the file-transfer interface. In a Git repository, pull builds the imported web history under `refs/librepaper/main` and merges it into the checked-out branch. Push reads eligible working-tree files and sends changes to the web project.
- `crates/librepaper-sync/src/pull.rs` discovers ordered named versions. It can also fetch `/project?unsuggested=1` as a `Current version` commit with `skip_if_unchanged`, but the current flow does not reliably append that head snapshot after all unseen named versions.
- The pull state has distinct `pulled`, `fetched`, `pushed`, `head`, `files`, and `synced` meanings. A fetched chain can be saved before a Git merge. Pushed labels can be adopted when the pull later sees those same versions. These semantics must remain distinct.
- `web_changed` in the pull and push paths currently checks named-version sequence progress. It can therefore miss an unnamed accepted edit at the current project head.
- `librepaper push` currently generates a request key for the POST and saves the resulting label/state only after a successful response. A lost response can leave the caller unsure whether the web project accepted the change.

### 3.2 Server request contract

- `librepaper_base::util::new_request_key()` creates a first-seen key shaped like `v2.<milliseconds>.<32-hex-nonce>`.
- Key validation is in `crates/librepaper-room/src/room/agent.rs`; project/principal scoping is constructed in `crates/librepaper-server/src/server/source_change.rs`; replay checks for an existing label in `crates/librepaper-room/src/room/source_change.rs` before preparing a new source change. This allows a retry to retrieve an existing result even after the project head moved.
- Source changes use `UnchangedDependencies`: touched paths are checked against their expected values. This is not a whole-project compare-and-swap. Unrelated edits may arrive while the request is in flight and remain for a later pull.
- Replayed outcomes depend on the server retaining the corresponding label/receipt. History trimming can remove that evidence. A client must not claim unconditional exactly-once behavior.
- `GET /project?unsuggested=1` opens the room, builds the document projection, serializes every text, and only then computes its etag. A conditional request saves bandwidth, not the rebuild.
- A document with no socket subscribers and an empty buffer is retired at the next housekeeping sweep. An unattended poll therefore rebuilds the document from its compaction base and log tail on every call.
- The current scheduled pull reads only `/api/config` and one page of `/history?named=1`, two indexed queries. Taking the project read of section 4.1 on every run would turn that into a cold rebuild and a full download per run per connected project.
- `GET /history` already opens the room. Opening reads the log head without building the document; `LogState.head_vector` is the log vector plus buffered batches.

### 3.3 Git panel and folder UI

- The web-project Git panel has repository Pull/Push actions. In the desktop app it works with an app-managed checkout and offers native Git operations as well.
- The local-folder side panel combines folder-to-web Pull/Push with native Git actions. Their direction changes with context, which makes a bare “Pull” or “Push” hard to interpret.
- `web/src/lib/reader/web-connection.svelte.js` currently has History naming workarounds that make unnamed current-head changes visible to the Git transfer flow. Once core pull imports the current accepted head, remove those workarounds so transfer does not depend on automatic History naming.
- `SPEC-sync.md` defines folder-to-web transfer as two actions. This proposal preserves that design and does not move its algorithm into the web-project Git panel.

### 3.4 Action

- `tools/sync-action/sync.sh` uses separate commands for separate events. A repository push runs CLI push toward the web project. Schedule/manual runs pull from the project and merge into Git. A rejected push can trigger the existing pull/PR conflict path.
- State is carried in `refs/librepaper/state`; imported web history is carried in `refs/librepaper/main`. The Action publishes those refs as part of its existing workflow.
- The current named-version pull path does not continuously capture an unnamed current draft. The current-head import in this proposal fixes that data gap without changing the Action into one combined operation.
- The existing first-run Action setup initializes both directions before settling into its event-specific push/pull behavior. Preserve that documented initialization path; it is not a promise that every later run is bidirectional.
- The documented schedule is every 15 minutes: 96 runs a day per repository, on the order of 2,000 to 3,000 GitHub Actions minutes a month, which exceeds the free allowance of a private repository. GitHub also delays and sometimes skips frequent cron jobs, so 15 minutes is not a reliable cadence in any case.

## 4. Proposed pull semantics

### 4.1 Ordered import

For each pull, fetch and append unseen named versions in their existing sequence order. After those named pages have been read, read the current accepted project snapshot from `/project?unsuggested=1`, unless the head identity returned with the history page equals the one recorded in state (section 4.5), in which case there is nothing new to read. If its eligible file tree differs from the resulting chain tip, append it as a `Current version` commit with `skip_if_unchanged: true`. If the trees are equal, do not create another commit. Compare with the actual chain tip, which may already be a prior current snapshot, not merely the newest named version. Use the existing project response/digest and file records; this proposal does not require a new server snapshot coordinator or high-water API.

The resulting internal chain is therefore:

```text
last imported base
  -> unseen named version A
  -> unseen named version B
  -> current accepted snapshot, only if different
```

The current snapshot is a transfer checkpoint, not a claim that the user saved a named version. The named versions remain visible in their original order and retain their source metadata. A snapshot that differs from the newest named version captures current accepted edits without requiring the user to bookmark or name them.

The same import rules apply to a plain folder and to a Git repository. In a repository, Git still performs the merge into the selected branch. In a plain folder, existing per-file conflict choices remain available. Read the named pages before reading the current head so a later named version is not imported after an older captured head. Do not apply the current snapshot before unseen named history: doing so can reorder the imported file evolution and misstate provenance.

### 4.2 Cursor and adoption invariants

- `pulled` continues to mean the newest named-version sequence imported. A synthetic current snapshot has no named sequence and must not advance this cursor.
- `fetched` continues to identify the fetched chain tip and the file records associated with the project snapshot, until a repository merge adopts it.
- `pushed` continues to map a local Git commit to the web label it created. When pull later discovers that label, adopt the existing commit where current behavior permits and consume the mapping. Do not replay it as an unrelated imported commit.
- `head` continues to identify the local Git commit representing the last accepted sync base when available.
- `files` and `synced` retain their existing meanings for change detection and deletion history.
- Persist the fetched chain/cursor before attempting the merge, as today. Do not advance `files` to a fetched repository state until the merge succeeds.
- Do not infer that a file was deleted on first link merely because it is absent from the other side. Deletion requires an established prior base and follows existing change detection.

### 4.3 Exclusions, line endings, and suggestions

- Use the same project path/type allowlist, asset handling, and `.gitattributes` / `-librepaper` behavior as the current transfer code. Keep the `-librepaper` attribute rule unconditional; it is part of the Git transfer contract.
- Preserve existing text normalization and Git line-ending handling when comparing and importing files; a CRLF checkout must not cause repeated snapshot commits for unchanged text.
- Excluded paths are not imported, exported, or deleted by LibrePaper file transfer. This does not constrain ordinary Git commits: a user may commit `.github` or other repository-only files and Git handles them normally.
- Hash and compare the accepted projection from `/project?unsuggested=1`, not an unresolved suggestion projection. Do not treat a pending suggestion as an accepted file change.
- A refused suggestion remains actionable and visible. The transfer must not mark its changes as completed or silently skip it.
- Local render outputs remain excluded only where the desktop folder integration already knows them. Plain CLI and Action flows must not claim to discover every generated output on a user's machine; use the documented ignore/attribute rules.

### 4.4 `web_changed`

Update both pull and push `web_changed` reporting. Return `true` if either (a) named sequence progress exists beyond `pulled`, or (b) the current accepted eligible tree differs from the last accepted snapshot represented by state. It must compare stable file identity (path, kind, digest), not only a History sequence. Check the head identity of section 4.5 first. Only when it differs from the recorded one, read the project and compare file identities against the imported chain tip/state, then record the new identity. Repeated checks after importing a current snapshot must not report a change solely because that snapshot was not a named version, and a quiet poll must not read the project texts at all.

Unknown or unreachable state is not zero changes. Return an error/unknown status that the UI can explain, rather than `false`. The UI may report “Could not check the project” and offer retry.

### 4.5 Head identity and read cost

The history list response gains one field, `head`: an opaque string that identifies the document's accepted state, buffered updates included, compared by equality only. Implementation: a digest of `LogState.head_vector`, read from the room the handler already opens. It changes on every accepted edit, suggestion accept or reject, and asset change. It does not build the document and adds no route, so Edit-token admission is unchanged. Do not reuse the project read's etag for this purpose: that etag is computed after the projection is built, and it also covers title, format, and pending-change offsets for the browser reader.

Client rules:

- The sync state gains an optional `web_head` field: the identity current when the project tree was last read or written.
- Pull reads one history page as today, records the `head` it returns, and compares it with `web_head`. Equal, with no unseen named versions: the pull is up to date and reads nothing else. Different or unset: proceed with the named import and the project read of section 4.1, then set `web_head` to the identity that history page returned. Read the history page before the project, never after, so an edit landing between the two reads is caught by the next poll instead of being recorded as seen.
- Push clears `web_head` after a successful send, because the server's head moved. The next pull reads the project once and records the identity again. Re-reading the history page after the push and recording its identity is an acceptable alternative.
- A response without `head`, from an older server, falls back to the project read on every pull. Never treat the absence as unchanged.
- Desktop status polls use the same comparison and never read the project texts for status.

Cost after this change: a quiet scheduled run costs the server one catalogue read for the document row, one log head read, and one indexed label query. The project is read only when the head moved, which is when there is work to do.

Optional backstop, not a work package: a per-token rate limit on `GET /project`, so a misconfigured workflow cannot rebuild a document every minute.

## 5. User experience and terminology

### 5.1 Separate the two products

The local-folder **Sync with LibrePaper** surface remains the home for linking a folder to a web project. It retains its existing two actions:

- **Get changes from web** imports the project into the folder.
- **Send changes to web** publishes eligible folder changes to the project.

Do not add a third bidirectional folder Sync action in this phase. This preserves `SPEC-sync.md` and keeps folder conflict decisions in the current flow.

The **Git** panel is for a Git repository connection. The web-project panel uses:

- **Get repository changes** imports repository changes into the web project and its History.
- **Send project changes** for web-project files to the selected repository branch.

The exact direction must appear in status and confirmation text. Avoid a standalone Pull or Push in these surfaces.

### 5.2 Native Git controls

Native Git controls operate between the local repository and its Git remote, not between Git and the web project. Preserve the intentional commit UI. Label actions with that destination, for example **Commit**, **Pull from remote**, **Push to remote**, and **Fetch from remote**. If retaining a convenience action, call it **Sync with remote** and show its steps explicitly: commit, pull from remote, then push to remote. Never call a cross-system transfer and a native Git operation simply “Sync” in the same panel.

Do not silently commit unselected files as part of a transfer. If native Git cannot proceed because the working tree or branch state is unsuitable, explain the concrete Git action the user must take.

### 5.3 Status

Show the selected project, repository, and branch in the Git panel. Show per-direction status such as “Repository changes available” or “Project changes available”; do not show a project-global “connected” badge. Connection is per computer, and the browser has no access to a machine-local clone or credentials.

After a completed transfer, distinguish “Imported from repository” from “Sent to repository” and identify the destination branch. If only one destination/action succeeded, state that plainly and provide the relevant retry action. Do not infer global completion from a History entry created by another computer.

### 5.4 First-link preview

Before a first import can replace or conflict with existing local files, offer a preview with counts and path lists for:

- paths present only in the project;
- paths present only in the destination;
- paths present on both sides with different contents;
- paths present on both sides with equal contents;
- paths blocked by an error that prevents transfer, with the reason;
- paths excluded by transfer rules.

The preview is informational. The CLI real pull re-fetches current inputs and recomputes the plan, then follows the ordinary pull and conflict behavior; it does not enter a repeated preview-confirmation loop. Desktop may show a changed destructive or conflicting inventory for confirmation before writing. The preview does not bind the project, advance cursors, write branch refs, or modify destination files.

## 6. Branch binding

### 6.1 Binding rule

Each Git sync state records the project identity and one target branch. The first real connection must select the branch explicitly. The system may propose the currently checked-out branch as a default, but the user confirms it before the first write. A preview does not bind.

Every later operation verifies the recorded project and branch. If the checkout is detached, or the currently selected branch differs from the pinned branch, refuse before reading or writing synchronized files. Explain the recorded branch and offer to check it out. Never switch branches automatically. An unborn but named branch may be explicitly bound by its symbolic branch name; detached HEAD is not a branch and must be checked out before binding.

Ordinary Git branches remain available for experiments and review. They are not automatically synchronized. The user merges changes into the pinned branch through normal Git operations, then runs the transfer.

### 6.2 State placement and rebinding

Keep state in the existing store: a plain folder uses the per-directory state file; a Git repository uses `refs/librepaper/state`. Add a branch field to the serialized state, with old state interpreted as unbound until explicit first binding. No server database or cross-computer registry is added.

The Git state ref is repository-wide. Before using it, validate both the web project identity and the pinned branch. If either differs, stop with an actionable mismatch message; do not overwrite another project's baseline. This phase does not silently rebind or clear state. A later explicit reconnect flow may offer a different branch or project only after reviewing and preserving/recomputing the baseline safely. Switching branches in one repository does not require a permanently separate repository, and native Git branch commands remain unaffected.

Different computers can still connect the same project to different repositories or branches. No project-global uniqueness is enforceable without a server registry, which is out of scope.

## 7. Web push pending request

### 7.1 Record and ordering

Add an optional pending web-push record to the existing state format. Keep it limited to the information required to replay the same logical request and apply its result:

```text
PendingPush {
  request_key: string,
  actor_identity: stable authorized principal identity or nonsecret credential fingerprint,
  semantic_request: enough data to replay the original operation,
  success_effects: {
    upsert_files: path -> FileRecord,
    deleted_paths: [path],
    candidate_commit: optional commit captured before the request
  }
}
```

The semantic request includes the intended paths, expected base values, uploaded asset identities, and attribution/message fields needed by the existing API. `success_effects` are the exact local state changes to apply after a success or replay; they are not recalculated from the current worktree. The operation key is generated once with `new_request_key()` and retained for that pending attempt. No additional intended-change digest is needed if the persisted semantic request and effects are sufficient to resume.

Before recovery, perform only checks that do not inspect new file content: validate the project/actor authorization and confirm the checkout is on the pinned branch. If the branch differs, stop and ask the user to switch back before resuming. A pending operation is not replayed from a different branch or by a different authorized principal. Never persist raw credentials. Use the stable principal identity already available to the caller; if none is available, bind pending state to a nonsecret fingerprint of the credential and conservatively refuse if the credential changes. Do not add an identity endpoint for this purpose.

The push flow is:

1. Load state and check for a pending request before scanning the working tree, planning new changes, checking stale file hashes, or returning an early “nothing changed” result. Branch and actor authorization checks above may run first.
2. If one exists, replay that pending semantic request with its original key and original actor. Do not include later local edits in this request.
3. If none exists, scan and plan as today. Capture the candidate commit and success effects. Upload required binary assets before persisting pending, then build the request and persist pending in the existing State store before POSTing it.
4. On a successful response or replay, apply only the saved success effects to `files`, `synced`, `pushed`, and `head`, clear pending, and save the finalized state. Record the returned label against the saved candidate commit in `pushed`; set `head` from that captured commit when present. Leave the named-version `pulled` cursor unchanged. Never derive these effects from a changed working tree.
5. Only after finalizing the pending attempt may the same command scan for later local edits. Those edits remain new work; they are never added to the prior request's completion record.
6. On a definitive refusal such as changed dependencies or a pending suggestion, clear the attempt only after the refusal is known and preserve current actionable refusal behavior. A transport failure or ambiguous server result leaves pending in state for retry.

If an uploaded binary is unavailable when reconstructing the original request, do not substitute the current file at that path. Stop with an actionable recovery error and retain pending state for reconciliation.

This requires StateStore support for durable save in both FileStore and GitRefStore before the POST. It does not add a server transaction or require a whole-project snapshot compare-and-swap.

### 7.2 Replay contract and limits

The server recognizes the operation key scoped to the project and authorized principal, and looks up an existing label before preparing a new source change. The key alone is not a transferable receipt, and raw JSON byte equality is not the replay contract. Retrying requires the same pending semantic request and same authorized principal; keep the current per-path `UnchangedDependencies` checks for first acceptance. There is no receipt-query endpoint in this phase.

Recovery is bounded:

- The pending record must survive in the local state file or repository state ref.
- History trimming can remove the label used for replay, but the client has no endpoint that reports “receipt absent.” A changed-dependency refusal is detectable and must be shown with a reconcile path. If the receipt expired but the same operation is accepted again, the client cannot always distinguish that from a replay; this is not an exactly-once guarantee.
- The same authorized principal and pinned branch must be available when resuming. Do not apply a pending request while on a different checkout branch or under another account.
- Do not mint a new request key for an ambiguous pending attempt. A new key is appropriate only after the prior attempt has a definitive outcome or the user reconciles after the receipt is unavailable.
- This is not a guarantee for a fresh runner whose state was never exported. For the Action, a normally completed run can publish updated state refs; an abrupt runner loss before state export is explicitly not covered in this phase.
- Same-call retries and persisted pending retries are supported. Global deduplication across independent machines or concurrent Action runs is not.

Pull, link, and unlink must not clear, finalize, or advance a pending push baseline. If one is present, stop and instruct the user to run `push` to recover it first. An Action schedule/manual pull follows the same rule: it reports an interrupted pending push and exits with the recovery instruction rather than implying that every event can resume every operation.

### 7.3 State compatibility

This is an unreleased change. Extend the existing serde state with optional fields and defaults. Do not add a compatibility flag, dual-write window, or migration framework. Old records load as unbound with no pending request. A pending request is not fabricated from a Git commit or reconstructed by guessing a request key.

## 8. CLI and desktop integration

### 8.1 CLI

Keep `pull` and `push` as the two transfer commands. Add only the focused options needed here:

```text
librepaper pull [PROJECT_URL] [--dir DIR] [--no-merge] [--mine PATH ...] [--theirs PATH ...] [--branch BRANCH] [--preview]
librepaper push [PROJECT_URL] [--dir DIR] [--message MESSAGE] [--author AUTHOR] [--title TITLE] [--branch BRANCH]
```

`--preview` is valid on the first-link pull path and exits after displaying project-only, local-only, different, equal, excluded, and blocked paths. The preview path must bypass `connect()` because the current `connect()` saves link state. It may read project data and create disposable local planning data, but it does not save a connection, alter state refs, change a branch, or write project files. Real pull always fetches again and rechecks, then follows normal pull/conflict behavior without a repeated preview loop.

For a repository, `--branch` confirms or explicitly establishes the pinned branch. An unbound repository requires this option on its first real transfer; Action and desktop callers pass the configured branch. With a saved branch, omission uses the saved value only if the checkout is on that branch. A mismatch or detached HEAD is an error. Do not reinterpret `--branch` as permission to silently switch branches. Reject `--branch` for a plain folder with a clear explanation.

Do not add `librepaper sync`, `--continue`, `--status`, `--json`, or a folder-web-only variant in this phase.

### 8.2 Desktop

Reuse the same CLI/sync crate behavior rather than creating a second planner. The web-project Git UI must flush its own editor changes before capturing a current project snapshot. It cannot promise to capture another browser's offline or unsent edits.

Use the preview planner for first connection. Show project-only, local-only, different, equal, excluded, and blocked paths. On confirmation, perform a fresh pull; if a destructive or conflicting part of the inventory changed, show the revised inventory for confirmation. Routine non-destructive changes use normal pull behavior. Provide explicit branch confirmation and mismatch recovery. The local-folder Sync surface retains its existing two actions and conflict choices.

If a web push is pending, desktop pull/link/unlink must leave it intact. A folder user can choose **Send changes to web** to recover it. In the web project's Git connection, offer **Retry interrupted transfer** on the error; it calls the existing web-push recovery path before restarting any outer connection sequence. Do not restart that sequence at a pull that would be blocked by the same pending request.

The browser-only web app may show project History and setup guidance, but it cannot claim a machine is connected or report that a local repository transfer completed. Forge credentials remain with the user's Git tooling or the desktop process.

## 9. GitHub Action

Keep the Action's current first-run initialization, event split, and state-ref mechanism. After initialization, update the CLI behavior so its repository-to-web push and web-to-repository pull both account for the current accepted project snapshot as specified above.

The Action configuration selects the branch explicitly. It must check out that branch and verify that the configured target matches it. A missing project link or unusable LibrePaper token fails with a direct setup error. Action configuration itself authorizes unattended execution; do not add an `initialize: true` gate.

Example workflow shape (the Action version below is a placeholder until the first release implementing this spec):

```yaml
name: LibrePaper files
on:
  push:
    branches: [main]
  schedule:
    - cron: "0 * * * *"
  workflow_dispatch:

concurrency:
  group: librepaper-${{ github.repository }}-main
  cancel-in-progress: false

permissions:
  contents: write
  pull-requests: write # only needed by the existing conflict PR path

jobs:
  transfer:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: main
          fetch-depth: 0
      - uses: LibrePaper/sync-action@vNEXT # replace with released immutable version
        with:
          project: https://app.librepaper.org/docs/<slug>
          token: ${{ secrets.LIBREPAPER_TOKEN }}
          branch: main
```

The schedule exists only to pick up web edits made without a Git push. Hourly is the documented default. A team that needs faster pickup may shorten it, knowing that every run spends GitHub Actions minutes and that, until section 4.5 is implemented, every run also costs the server a full project read. Documentation must not show an interval under 15 minutes.

The Action continues to use the runner's `GITHUB_TOKEN` for repository writes and the user's LibrePaper Edit token for project writes. A protected branch may reject the Action's direct push. Report repository and web completion separately and accurately: a rejected repository push does not undo a web write that already succeeded. Never force-push the target branch or bypass its protection. Existing conflict PR behavior remains the recovery path where it applies.

If a previous push left pending state, a later push event may recover it when the failed run's ordinary EXIT state-ref export succeeded. A schedule/manual event that reaches pull while pending must stop with an instruction to run the push recovery path. Recovery is not assured when export is suppressed, as during incomplete first-run initialization or a rejected target-branch push, when export fails, or when the runner disappears before export. Keep those limits explicit; this phase does not change the existing state publication protocol.

The design must not depend on the Action's own repository push triggering another workflow run. Each event must finish its intended transfer or report what remains incomplete.

## 10. Implementation work packages

Estimates below are planning ranges, not measurements. They include production and test changes and assume the existing two-command architecture.

| Package | Ownership and concrete work | Rough production and test estimate |
|---|---|---:|
| A. Current-head import | `crates/librepaper-sync/src/pull.rs`, related chain/discovery code and crate tests; remove History naming workaround from `web/src/lib/reader/web-connection.svelte.js`. Import unseen named labels first, append current accepted snapshot only when different, preserve sequence/adoption state, and extend pull/push `web_changed`. Add the `head` field to the history list response in `crates/librepaper-server/src/server/history.rs`, the `web_head` state field, and the skip rule of section 4.5, so a quiet pull makes no project read. | 330-600 LOC |
| B. Destination wording | Reader Git and folder Sync components/controllers and focused UI tests. Keep folder actions separate; distinguish repository-to-web-project transfer from native remote actions and preserve the intentional commit UI. | 150-350 LOC |
| C. Branch pinning | `crates/librepaper-sync/src/state.rs`, Git branch inspection, CLI option parsing in `crates/librepaper/src/cli/sync.rs`, desktop connection UI/bridge as needed, and crate/CLI tests. Add optional branch binding, validate mismatch/detached state, and refuse silent switching. | 150-350 LOC |
| D. Pending push recovery | `crates/librepaper-sync/src/state.rs` and `push.rs`, CLI handling in `crates/librepaper/src/cli/sync.rs`, Action state handling, server-contract tests where needed. Save actor, request, captured commit, and exact success effects before POST; recover before new content inspection and finalize from saved effects. | 300-550 LOC |
| E. First-link preview | CLI pull planner/output in `crates/librepaper/src/cli/sync.rs`; desktop bridge in `crates/librepaper-companion/src/local/service/{sync,connection}.rs` as needed; preview UI in reader Git components; CLI/desktop tests. Bypass state-saving `connect()` in preview and recheck on execution. | 250-500 LOC |
| Cross-cutting Action and docs | `tools/sync-action/sync.sh`, Action metadata, `docs/git.md`, relevant clarifications in `SPEC-sync.md`, and focused workflow coverage. Keep event direction, state refs, and pending-push recovery instructions explicit. Change the documented default schedule to hourly and explain its cost. | 150-300 LOC |

Package D must use the actual existing StateStore serialization and source-change replay flow. If this requires a broader durable transaction or a new server receipt contract, stop that package and return for a scoped design update rather than quietly adding a coordinator. These packages share files, so parallel implementation needs explicit ownership or sequential integration. Revisit any estimate that grows materially before expanding the scope.

### Likely source map

- Pull/import state: `crates/librepaper-sync/src/pull.rs`, `state.rs`, chain/discovery implementation, and crate tests.
- Request creation/replay: `crates/librepaper-sync/src/push.rs`; request key generation in `crates/librepaper-base/src/util.rs`; server replay and path-dependency validation in `crates/librepaper-room/src/room/source_change.rs`.
- CLI: `crates/librepaper/src/cli/sync.rs` command argument definitions, pull/push dispatch, and output rendering.
- Desktop bridge: `crates/librepaper-companion/src/local/service/sync.rs` and `connection.rs`, only where branch/preview data must cross the bridge.
- Web UI: `web/src/components/reader/Git.svelte`, Git child components/controllers, `Reader.svelte`, `web/src/lib/reader/web-connection.svelte.js`, and the separate local-folder Sync components. Keep responsibilities aligned with existing component boundaries.
- Action and documentation: `tools/sync-action`, `docs/git.md`, and the relevant portions of `SPEC-sync.md`.
- The one server change is the `head` field on the history list response (section 4.5). Other History/source-change server changes should be unnecessary unless inspection proves the existing current projection or replay response lacks required data. Prefer the existing `/project?unsuggested=1`, operation key, and receipt behavior.

## 11. Acceptance matrix

| Case | Expected result |
|---|---|
| One unseen named version, current tree identical | Import the named commit once; no extra snapshot commit. `pulled` advances to that named version. |
| Unseen named versions plus later unnamed accepted edit | Import named versions in order, then one `Current version` commit for the differing accepted tree. `pulled` remains the newest named sequence. |
| No new named version, current accepted tree differs | Import one current snapshot and report web changes available. |
| Repeated pull with no changes, including a CRLF checkout | No extra snapshot commit; current-head status becomes unchanged. |
| Scheduled pull, head identity unchanged | One history read; no project read, no document rebuild, no commit. |
| Browser edit with no named version and no render | Head identity changes; the next pull reads the project once and imports one current snapshot. |
| State has no recorded identity, or the server answers without `head` | Pull reads the project once; the identity is recorded only when the server supplied one. |
| Push succeeds, then pull | `web_head` is cleared or re-read; the pull does not report the pushed files as a web change. |
| History was trimmed, then current accepted files changed | Preserve existing trim recovery, capture current files, and do not invent a named sequence for the snapshot. |
| Pull discovers a label created by this repository's earlier push | Preserve existing commit adoption and consume its `pushed` mapping where applicable. |
| Pending suggestion differs from accepted projection | Do not import the suggestion. If push is refused for a suggestion, keep it actionable and do not mark it sent. |
| Pull into a plain folder with a same-path conflict | Existing whole-file choice flow remains. Preview does not write; actual pull rechecks. |
| Pull into a Git repository with disjoint edits | Git merges normally into the pinned branch. |
| Pull into a Git repository with overlapping edits | Existing merge conflict is reported for normal Git resolution; no implicit side wins. |
| Checkout is detached or differs from pinned branch | Refuse before synchronized writes and name the expected branch. |
| First transfer on an unborn but named branch | Explicit binding succeeds by symbolic branch name without requiring an existing commit. |
| First-link preview | Show project-only, local-only, different, equal, blocked, and excluded paths. Do not bind or write. Actual pull recomputes and uses normal conflict handling. |
| Web push response is lost after server acceptance | Persisted pending request retries with its original key; retained receipt is returned and state finalizes without a duplicate label. |
| Local files change after pending request was persisted | Recover/finalize prior request first. Later edits remain pending work and are not included in prior effects. |
| Pending replay receives a definitive dependency refusal | Report the refusal without claiming the earlier attempt succeeded or never happened. Clear the refused attempt without advancing its baseline so normal pull/reconciliation can proceed. If a receipt has expired but replay is accepted, the client may be unable to detect that expiration. |
| Pending replay has another transport failure or changed credentials | Retain pending and its original effects; do not scan newer files into the old request or generate a new key. |
| Saving pending fails before POST | Do not send the source-change request. |
| Pull, link, or unlink is requested while push is pending | Stop without changing the pending baseline; direct the user to recover with push first. |
| Action push event | Keep repository-to-web direction and current conflict handling. No reliance on a second workflow triggered by its own token push. |
| Action schedule/manual event | Keep web-to-repository direction; current accepted snapshot is considered after named versions. |
| Protected branch rejects a push after a web write succeeded | Report web success and repository failure separately; do not imply rollback, force-push, bypass protection, or claim parity. |
| Folder Sync panel | Continue to show Get changes from web and Send changes to web as separate actions. |
| Native Git panel | Labels identify remote direction and do not imply web-project transfer. Convenience Sync shows its commit, pull, and push steps and preserves the existing intentional commit UI. |

Future implementation should add or update focused crate tests, CLI integration coverage, desktop/UI tests, and Action workflow coverage for these cases. Per repository instructions, the implementing/reviewing parent runs the relevant crate `cargo check` and `cargo nextest` commands and the full `make test` only after combined review. No checks are part of this specification-authoring task.

## 12. Deferred options and their tradeoffs

These are possible later projects, not implied requirements of this phase. Their frequency and value have not been measured. Estimates are incremental production plus test LOC, not measurements.

### One bidirectional Sync operation

**Need:** Users may want one action that captures both endpoints, merges, and publishes to both. The current two commands and event-specific Action can require a second action to complete a round trip. **Frequency:** unknown. **Rough addition:** 700-1,300 LOC for shared orchestration, desktop/CLI/Action integration, and failure-state tests. **Tradeoff:** simpler routine action, but a larger cross-platform state machine and more partial-completion states. Keep the existing two-command contract until evidence justifies this expansion.

### Custom conflict comparison and combined editor

**Need:** A user may want LibrePaper to show a three-way comparison and edit the combined result rather than resolve through Git or choose a whole file in the folder flow. **Frequency:** unknown. **Rough addition:** 1,500-4,000 LOC including UI, persistence, and tests for a workspace backed by Git; a custom Rust text merger would need its own correctness case and an additional rough 1,000-3,000 LOC estimate. **Tradeoff:** a more guided workflow, but a new merge surface with text/binary semantics and additional maintenance. This phase retains Git's merge and existing folder choices.

### Fresh-runner journal recovery

**Need:** A runner can accept a web push, then disappear before exporting its repository state ref; a fresh runner may not know the pending request. **Frequency:** unknown. **Rough addition:** 400-900 LOC for a stronger remote journal, lifecycle/retention handling, and recovery tests. **Tradeoff:** better recovery from abrupt CI loss, but more state and server/remote protocol complexity. This phase supports recovery only when pending client state survives.

### Pull-request lifecycle redesign

**Need:** Teams may want conflict proposals to handle all first-link histories, merge methods, and concurrent edits through one carefully managed PR lifecycle. **Frequency:** unknown. **Rough addition:** 600-1,200 LOC across Action code, provider APIs, and workflow tests. **Tradeoff:** more automated review, but provider-specific behavior and a larger automation surface. This phase retains the existing event-specific Action and conflict PR path.

### Plain-folder merge baseline

**Need:** A folder user may want line-level comparison and a combined text result rather than the current whole-file conflict choice. **Frequency:** unknown. **Rough addition:** 350-800 LOC for persisted base content, merge logic, UI, and tests. **Tradeoff:** finer resolution, but more local state and text/binary edge cases. This phase preserves existing folder behavior.

## 13. Release boundary

This is an unreleased change. Implement it as one hard cutover for the in-scope behaviors. Do not add compatibility flags or a gradual dual-mode period. Preserve existing named-version history and sound state; do not reseed a repository from scratch or delete a valid baseline to make branch binding pass. Old serialized state remains readable through optional fields and defaults. If a state cannot be safely bound to the intended project/branch, stop with a concrete recovery instruction.

The implementation is ready for release when the five in-scope work packages satisfy the acceptance matrix and the normal centralized review/check process. Deferred projects do not block this phase.
