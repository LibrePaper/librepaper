# SPEC: LibrePaper as a git remote, through the companion

Status: proposed 2026-10-14, not started. This replaces the earlier
project-folder sync design in full.

## Summary

The companion ships a git remote helper, `git-remote-librepaper`. A user adds
a LibrePaper project as a remote and uses plain `git pull` and `git push`.
Fetch turns the project's current files into a commit. Push turns the
pushed commits into minimal text edits and applies them on the server. A
push is refused only when it overlaps edits that others made since the
user's last pull. Overlapping work is then merged by git on the user's
machine, with ordinary conflict markers.

The design is conservative on purpose:

- The companion never acts as a Loro peer. It sends plain text patches, and
  the server turns them into document edits.
- Concurrent edits to the same lines are never merged by Loro. Git handles
  them.
- No browser tab is needed after the remote is set up.
- The server holds no third-party git credential and builds no git objects.

## Goals

1. `git clone`, `git pull` and `git push` against a LibrePaper project, from
   any desktop where the companion is installed.
2. A push lands in open editors live, like a coauthor's typing.
3. Comments and pending tracked changes stay attached to text that a push
   does not touch.
4. A push is refused only for real overlap, never because someone typed
   elsewhere in the same file.
5. Every refusal names the file and the reason, and says what to run next.
6. An existing git repository can be connected to an existing project with
   one command.

## Non-goals for v1

- Use without the companion, for example from CI or a lab server. See
  "Later".
- Commit ids that match across machines. See "Known limits".
- Named versions as commits or tags.
- More than one branch. The remote has exactly one branch, `main`.
- Git LFS and submodules.
- `git push -o review` in the helper. The server route supports
  `"publish": "suggestions"` from step 3, so adding the flag later is
  helper work only.

## User experience

### Connecting

Plain git is enough. No `librepaper` subcommand is required:

```sh
git clone librepaper::<slug> thesis
# or, in an existing repository:
git remote add librepaper librepaper::<slug>
git pull librepaper main
```

- **Consent on first use.** When the helper has no grant for the project
  on this computer, it opens the browser to a consent page and waits. The
  page names the project and the computer and asks "Allow git access to
  this project from this computer?" Accepting mints the grant, and the git
  command continues. See "Credentials".
- **Short URLs.** `librepaper::<slug>` means the project `<slug>` on the
  site this companion is paired with. The full form
  `librepaper::https://librepaper.org/p/<slug>` names the site explicitly.
  Both forms work everywhere a remote URL is accepted.
- **The `.librepaper` file.** Connecting writes a `.librepaper` file at the
  repository root that names the project. It is committed like any other
  file. A coauthor who clones the repository from GitHub runs one command
  with no arguments:

  ```sh
  librepaper git link    # reads .librepaper, adds the remote, joins the histories
  ```

  When the helper or `librepaper` runs in a repository that has a
  `.librepaper` file but no `librepaper` remote, it prints that command as a
  hint.
- **The helper commands.** `librepaper git clone <slug|url> [dir]` and
  `librepaper git link [<slug|url>]` wrap the git commands above. They also
  write `.librepaper`. `link` also joins an existing history, see
  "`librepaper git link` in detail".
- **The "Use with git" panel.** The project menu shows the commands above
  with the slug filled in. It lists the computers that hold a grant for the
  project, each with a Revoke button.

### Daily use

```sh
git pull librepaper main      # bring in everyone's LibrePaper edits
# edit locally, commit as usual
git push librepaper main      # send your commits to LibrePaper
```

- A push that applies prints `LibrePaper: applied 3 files (paper.qmd,
  refs.bib, fig/plot.png)`.
- Coauthors with the project open see the text change live and a notice
  that reads `<name> pushed 3 files from git`.
- `git pull` makes a merge commit when the user also has local commits.
  `git pull --rebase` avoids that, and the docs say so.

### Refusals

Every refusal is a git push error on the `main` ref, with one line per
problem and a final line saying what to do:

```text
! [remote rejected] main -> main (LibrePaper changed since your last pull)
  paper.qmd lines 40-52 were edited in LibrePaper since your last pull.
  Run: git pull librepaper main, then push again.
```

The full list is in "Push: refusal rules".

## Architecture

There are three parts.

1. **The helper.** `git-remote-librepaper` is the `librepaper` binary under
   a second name, installed next to it by every install method (installer
   script, Homebrew, Scoop). Git starts it whenever a remote URL begins with
   `librepaper::`. It runs as a short-lived process for one fetch or push.
   It reads the companion's credential store directly, so the companion's
   background service does not have to be running.
2. **The companion service.** It is needed only while a new grant is being
   approved. It confirms to the consent page that this computer started the
   request. See "Consent on first use". The helper starts the service if it
   is not running, the same way `librepaper` does.
3. **The server.** It gets a git grant, thin routes under `/api/git/v1/`,
   and one source-change core shared with the agent tools. See "Server
   apply".

The helper speaks git's remote-helper protocol with the `import` and
`export` capabilities. Git produces and consumes fast-import and fast-export
streams, so the helper never writes pack files or computes object ids by
hand.

## Credentials

Today the companion has no standing credential. An agent grant
(`POST /api/documents/{slug}/agent-token`,
`crates/librepaper-server/src/server/agent_auth.rs`) lasts five minutes. It
can only be minted by the signed-in document page and must name a link
capability. That cannot serve a `git push` made hours later with no tab
open. Git therefore needs its own grant.

### The git grant

- **Minting.** Grants are minted only through the consent flow below. The
  approval route follows the same rules as the agent-token route: cookie
  only, same origin, signed in.
- **Scope.** One account, one document, one computer. The grant records the
  computer's name for display. It does not name a link, because the grant
  belongs to the account, not to a share link.
- **Role.** Every request rechecks the account's current role on the
  document. Viewers and commenters can fetch. Only editors can push.
  Removing a person from the project ends their git access immediately.
- **Lifetime.** 180 days, extended by use. The project's "Use with git"
  panel and the account settings list grants, and both can revoke them.
  Signing out everywhere revokes every git grant.
- **Storage.** The server stores only a hash of the token. The companion
  stores the token in its credential store
  (`crates/librepaper-companion/src/local/credentials.rs`), keyed by site
  and document.
- **Token prefix.** `lpg_`, so secret scanners can tell it apart from other
  LibrePaper tokens.

### Consent on first use

Any helper request that finds no usable grant starts this flow. A revoked
or expired grant starts it too.

1. **Request.** The helper calls `POST /api/git/v1/requests` with the slug,
   the computer's name and a random 32-byte secret it keeps in memory. The
   server stores a hash of the secret and returns a request id. The request
   expires after ten minutes.
2. **Register.** The helper tells the local companion service the request
   id over loopback, using the companion's existing local protocol.
3. **Open.** The helper opens
   `https://<site>/git/approve?request=<id>` in the default browser and
   prints `Waiting for approval in your browser…` plus the URL, in case the
   browser did not open.
4. **Confirm the computer.** The consent page asks the paired companion on
   this machine, over the existing site pairing, whether it registered this
   request id. The page shows the Allow button only when the companion says
   yes. This blocks a phishing link: a request started on someone else's
   computer is not registered with the victim's companion, so the page
   refuses it with "This request did not come from this computer."
5. **Approve.** The person clicks Allow. The page calls
   `POST /api/git/v1/requests/{id}/approve`, which checks the cookie, the
   person's access to the project and the request's expiry, then marks it
   approved.
6. **Collect.** The helper polls `POST /api/git/v1/requests/{id}/token`
   with the secret every two seconds. Once the request is approved, the
   first call that presents the right secret receives the grant token. The
   request is then deleted, so the token is handed out once.
7. **Continue.** The helper stores the token in the credential store and
   finishes the original fetch or push.

Pressing Ctrl-C in the terminal abandons the request. An abandoned request
expires on its own.

When no paired companion answers in step 4, for example on a computer
reached over SSH, the page falls back to a typed code. The helper prints
an eight-character code, and the page asks the person to type it. A typed
code proves the person can see the terminal that started the request.

### Version handshake

Every helper request sends `LibrePaper-Git: 1`. The server answers
`426 Upgrade Required` when it no longer accepts that version, with a body
naming the minimum version and the update command for each install method.
The helper prints the message as the git error. The same header lets the
server log which helper versions are still in use.

The helper never updates itself. The update command depends on how the
companion was installed:

| Install method | Update command |
|---|---|
| Installer script | `librepaper-update` |
| Homebrew | `brew upgrade librepaper` |
| Scoop | `scoop update librepaper` |

The helper detects the method from its own install path. The companion
settings page shows "Update available" with the same command when the
server's minimum version, or the latest release, is newer than the
installed one.

## The `.librepaper` file

```toml
# Written by librepaper. Names the LibrePaper project this repository syncs with.
site = "https://librepaper.org"
project = "<slug>"
```

- It holds no secret, so committing it is safe.
- It is local-only: it is never uploaded into the project.
- `site` lets `librepaper::<slug>` resolve correctly for a coauthor whose
  companion is paired with more than one site.
- A repository with two LibrePaper remotes keeps the first one in the file.
  The second remote works, but `link` with no argument uses the file.

## Remote URL forms

| Form | Meaning |
|---|---|
| `librepaper::<slug>` | Project `<slug>` on the site named in `.librepaper`, or else the only site the companion is paired with |
| `librepaper::https://<site>/p/<slug>` | Project `<slug>` on `<site>` |

When the short form is ambiguous because the companion is paired with
several sites and no `.librepaper` names one, the helper refuses and lists
the full URLs to choose from.

## Remote state kept by the helper

The helper keeps per-remote state in the user's repository:

- `refs/librepaper/<remote>/main`: the last commit exchanged with
  LibrePaper, which is the last commit fetched or pushed.
- `.git/librepaper/<remote>/state.json`, which holds:
  - `commit`, the same commit id as the ref;
  - `project_digest`, the server's projection digest at that commit, or
    `null` when unknown;
  - `marks`, the path of the fast-import and fast-export marks file.

This state is the base for every push and the parent for every fetch. Git
never sends it anywhere.

## Fetch

### Server read

`GET /api/git/v1/{slug}/project` returns what
`GET /api/documents/{slug}/project` returns today (`schema`,
`project_digest`, `tree`, `texts`). It accepts the git grant instead of a
cookie, and supports `If-None-Match` on the digest. Binary files are read
from `GET /api/documents/{slug}/assets/{sha}`, which accepts the git grant
in the same way.

### Steps

1. Read `state.json`. If the server's `project_digest` equals the recorded
   one, report the recorded commit as `main` and stop. Nothing changed.
2. Download the project. Download each binary whose content hash is not
   already a blob in the repository.
3. Build the new tree:
   - every project file, at its project path;
   - every path in the recorded commit's tree that is local-only (see
     below), unchanged.
4. Write one commit through fast-import:
   - parent: the recorded commit, or none on the first clone;
   - author and committer: `LibrePaper <git@librepaper.org>`, dated at the
     fetch time;
   - message: `LibrePaper edits` plus a blank line and a trailer
     `LibrePaper-Digest: <project_digest>`.
5. Update the state ref and `state.json`.

### Local-only paths

A git repository often holds files the project should not, such as a
Makefile, data or CI configuration. Fetch must not delete them, and push
must not upload them.

- `.librepaperignore` at the repository root uses gitignore syntax.
- These paths are always local-only: `.git/`, `.gitignore`,
  `.gitattributes`, `.librepaper`, `.librepaperignore`, `.github/`, and
  `*.local`.
- Fetch carries local-only paths over from the parent commit unchanged.
- Push skips them.

### Line endings

Project text uses LF. Fetch writes LF. With `core.autocrlf` set, git
converts files in the working copy and converts them back on commit, so
committed blobs stay LF. Push converts any CRLF in a text blob to LF before
diffing, so a Windows checkout never shows up as rewriting every line.

## Push

### Inputs

Git sends a fast-export stream for `refs/heads/main`. The helper reads
three versions of every file:

- **base**: the tree of the recorded commit, which is what the user last
  received from LibrePaper;
- **ours**: the tree of the pushed commit;
- **theirs**: the server's current project, read fresh.

### Steps

1. **Branch check.** Refuse any ref other than `refs/heads/main`. Refuse
   ref deletion.
2. **Ancestry check.** The pushed commit must descend from the recorded
   commit. Otherwise refuse with "fetch first", even for `--force`.
   LibrePaper never rewinds.
3. **Collect changes.** Diff base against ours, skipping local-only paths.
   Intermediate commits are collapsed. Only the net change is applied.
4. **Validate.** Apply every rule in "Push: refusal rules". Any failure
   refuses the whole push.
5. **Read theirs.** Fetch the current project and its `project_digest`.
6. **Three-way check, per text file.**
   - Compute line hunks for base to ours and for base to theirs, with the
     same algorithm and adjacency rule as `git merge-file`. Two hunks
     conflict when they overlap or touch.
   - If any hunk in ours conflicts with a hunk in theirs, refuse, listing
     every conflicting file and line range.
   - Otherwise map each ours hunk onto theirs: its base line range,
     shifted by theirs' earlier hunks.
   - Inside each mapped hunk, compute a character diff between the theirs
     text and the ours text. Emit one patch per changed run, with byte
     offsets in theirs and the `exact` text currently there. Minimal
     patches are what keep comment anchors attached.
7. **Non-text changes.** Collect file additions, deletions and binary
   replacements. A binary file changed on both sides since base is a
   conflict and refuses. A file deleted in ours but edited in theirs is a
   conflict and refuses.
8. **Submit.** Send one change set (see "Server apply") naming the
   theirs digest of every touched file.
9. **Race retry.** If the server refuses because a touched file changed
   between step 5 and step 8, go back to step 5. After five attempts, with
   backoff of 0.2, 0.5, 1, 2 and 4 seconds, refuse with "the project is busy
   in that file, try again".
10. **Record.** Set the state ref to the pushed commit. Set
    `project_digest` to the digest the server returns, but only when theirs
    equaled base for every file. Otherwise set it to `null`, so the next
    fetch makes a commit carrying the merged result. Report `ok
    refs/heads/main`.

### Push: refusal rules

| Rule | Message names |
|---|---|
| Ref is not `main`, or is a deletion | the ref |
| Pushed commit does not descend from the last fetch | "fetch first" |
| A text file contains a line starting with `<<<<<<< `, `=======` or `>>>>>>> ` | file and line |
| A text file is not valid UTF-8 | file |
| A path fails the server's path rules (`validate_path` in `agent.rs`) | path |
| A file is larger than the project's per-file limit | file and limit |
| The push would exceed the account's storage quota | quota |
| Hunks overlap edits made in LibrePaper since the last pull | file and lines |
| The user is not an editor | role |
| The grant is revoked or expired | the reconnect command |
| The helper is too old | the update command |

The `=======` rule refuses only when a `<<<<<<< ` line also appears in the
same file, because a line of `=` is valid Markdown.

### Renames

A rename arrives as a deletion plus an addition. Comments anchored in the
old file are lost, exactly as when a file is deleted in the editor. Rename
detection can come later, if the server gains a rename operation that keeps
a file's identity.

## Server apply: one source-change core for agents and git

Agents and git pushes both turn a list of text patches into one document
mutation, with retries keyed by an operation id. The agent path already
does this (`Room::apply_agent_request`, `AgentPatchCommand`,
`validate_patches` in `crates/librepaper-room/src/room/agent.rs`). Git does
not get a second apply path. The agent path is generalized into one core
that both call, and both gain from every improvement to it.

### What exists and is reused unchanged

- **`PatchRequest`.** Byte-range patches with the `exact` text currently
  there, an operation key and a base tree digest.
- **`Consistency::UnchangedDependencies`.** A dependency with a
  `file_hash` refuses when that whole file changed. One dependency per
  touched file is exactly the per-file check a push needs. No new
  consistency mode is required.
- **Sequencing and replay.** `AgentPatchCommand` runs inside
  `Sequencer::command` under the transaction lock, and its replay check
  returns the first outcome for a retried operation id.
- **Authorization.** `authorize_document_mutation` and the recheck
  callback.
- **Tracked changes.** `document_propose` with `publish: "suggestions"` applies
  patches as tracked changes through `AgentTrackedCommand`.

### What changes in the room

1. **Rename to what it is.** `PatchRequest` becomes `SourceChange`,
   `AgentPatchCommand` becomes `SourceChangeCommand`,
   `apply_agent_request` becomes `Room::apply_source_change`, and
   `validate_patches` becomes `validate_source_change`. The module moves
   from `room/agent.rs` to `room/source_change.rs`. Agent-only code
   (operation key formats, MCP receipts) stays in `room/agent.rs`. Per the
   hard-cutover rule, there are no aliases for the old names.
2. **File operations.** `SourceChange` gains three lists next to
   `patches`:
   - `create`: a new text file with its path and text;
   - `delete`: a path, guarded by a whole-file dependency like a patch;
   - `put_binary`: a path and an asset hash already uploaded through the
     existing asset route.
   Validation covers them: valid paths, no creation over an existing
   path, no patch and deletion on the same file, and the storage quota.
   They apply in the same mutation as the patches, all or nothing.
   Agents get these operations too. An agent can then create a file, which
   today it cannot.
3. **Origin.** A trusted `Origin` value, set by the route and never read
   from the request body, replaces the hard-coded `automation: true`:

   | Origin | `automation` | Recorded source | Patch limits |
   |---|---|---|---|
   | `Agent` | true | `agent` | 100 patches, 256 KiB (today's values) |
   | `Git` | false | `git` | 5,000 patches, 8 MiB |

   The limits move from the constants `MAX_PATCHES` and `MAX_PATCH_BYTES`
   into the origin, so each caller keeps its own bound. The recorded
   source drives the editor notice ("pushed from git") and the activity
   record. Like the operation scope, `Origin` is kept out of the request
   digest, so it cannot alter a retry's identity.
4. **Tracked changes take a `SourceChange`.** `AgentTrackedCommand` accepts
   the same type, so any caller that can apply a change can also publish it
   for review. File operations cannot be tracked changes yet. A change that
   has them and asks for review is refused with that reason.

### The callers

- **MCP `document_apply`.** Unchanged for the model. It loads its stored
  candidate, builds a `SourceChange` with `Origin::Agent`, and calls
  `apply_source_change`. Views, signed range ids and candidates stay,
  because they protect a model from inventing offsets. They are the agent's
  front end, not part of the core.
- **Git push.** `POST /api/git/v1/{slug}/push`, authenticated by the git
  grant, editor role required. The helper is deterministic code, so it
  sends a `SourceChange` directly:

  ```json
  {
    "operation": {"epoch": "...", "id": "v2...."},
    "base_tree": "<project_digest the helper read>",
    "consistency": "unchanged_dependencies",
    "dependencies": [{"path": "paper.qmd", "file_hash": "<sha256>", "start": 0, "end": 0, "exact": ""}],
    "patches": [{"path": "paper.qmd", "start": 120, "end": 131,
                 "exact": "old words", "replacement": "new words"}],
    "create": [{"path": "notes.md", "text": "..."}],
    "delete": ["old.tex"],
    "put_binary": [{"path": "fig/plot.png", "sha": "<asset sha>"}],
    "message": "Chapter 2 draft",
    "publish": "apply"
  }
  ```

  The route parses the body, sets `Origin::Git`, and calls
  `apply_source_change`. With `"publish": "suggestions"` it calls the
  tracked-change command instead, which is how `git push -o review` works. The
  route holds no logic of its own beyond authentication and parsing.
  `message` is the last pushed commit's subject, used for the notice.

The response is the new `project_digest`, the applied paths and the
operation receipt.

### On the companion side

The agent bridge (`crates/librepaper-companion/src/automation/peer.rs`
and `automation/mcp.rs`) already mints operation ids, retries with the
same id after a dropped connection, and maps refusals to messages. The
helper reuses that client code. Only the credential differs: the git
grant instead of the five-minute agent grant.

### Improvements that serve both callers

These live in the shared core, so agents and git both benefit.

- **Range-level checks instead of whole-file checks.** A whole-file
  dependency refuses when anyone typed anywhere in that file. Agents hit
  this today: `document_apply` defaults to `exact_tree`, which refuses
  when anything in the project changed. The core can instead check each
  patch's range at the current head, by mapping its offsets from the base
  through the edits made since. It would then refuse only when the patched
  text itself changed. The base frontier is already recorded with every
  label (`frontier` in the label table). Mapping can use Loro cursors taken
  at the base and resolved at the head. Text is still never merged by
  Loro: an edit inside a patched range is still a refusal. This removes
  the busy-file limit for git and most "source tree changed" refusals for
  agents. It is a later step with its own tests, not part of v1.
- **One test suite.** Validation, replay, file operations and limits are
  tested once at the core. Each caller tests only its own parsing and
  authentication.

## Known limits

- **Commit ids differ across machines.** Each helper writes its own
  LibrePaper commits. Two people who both pull from LibrePaper and also
  share a GitHub repository get two commits for the same LibrePaper state.
  Git merges them cleanly, because both sides have the same content, but
  the history shows extra merge commits. Solo authors and people who share
  only through LibrePaper never see this. Matching ids would need the
  server to store commit metadata for every fetch and push. That is
  deferred until someone asks for it.
- **A very busy file can exhaust the retries.** A coauthor typing
  continuously in the same file can change it between the read and the
  apply on all five attempts. The push then refuses with "busy" and
  succeeds when typing pauses. Range-level checks in the shared core
  remove this limit. See "Improvements that serve both callers".
- **Desktop only.** The helper needs the companion's install and grant.
- **History is coarse.** One LibrePaper commit per fetch that saw a
  change. The detailed history stays in LibrePaper.

## Build order

Each step can be merged and shipped on its own.

1. **Server: grant and read.**
   - git grant table, the request, approve and token routes, revoke,
     listing;
   - grant authentication on `/api/git/v1/{slug}/project` and on asset
     reads;
   - version handshake;
   - revoke on sign-out everywhere and on role removal.
2. **Companion: fetch.**
   - `git-remote-librepaper` name and install for all three install
     methods;
   - remote-helper protocol with `import`;
   - per-remote state, local-only paths, line endings;
   - both remote URL forms;
   - consent on first use, including request registration with the
     companion service and the typed-code fallback;
   - `librepaper git clone` and `librepaper git link`, the `.librepaper`
     file, and the hint when a repository has the file but no remote.
   At this point clone, pull and backup to GitHub work.
3. **Server: shared source-change core.**
   - rename the agent apply path to `SourceChange` and move it to
     `room/source_change.rs`, with no behavior change for agents;
   - file operations, `Origin` with per-origin limits and the recorded
     source;
   - tracked changes accept a `SourceChange`;
   - the thin push route.
4. **Companion: push.**
   - `export` capability, ancestry check, refusal rules, three-way hunk
     check, patch generation, race retry, state update.
5. **Web.**
   - "Use with git" panel with commands and the grant list;
   - consent page at `/git/approve`, with the companion check and the
     typed-code fallback;
   - grants in account settings;
   - the "pushed from git" notice in the editor.
6. **Docs.** A `docs/git.md` page with the commands and every refusal
   message.

## `librepaper git link` in detail

Linking joins an existing repository's history to the project without an
"unrelated histories" error.

1. Refuse if the working tree has uncommitted changes.
2. Find the project. Use the argument when one is given. Otherwise read
   `.librepaper`. Refuse with "Name the project:
   `librepaper git link <slug>`" when neither exists.
3. Add the `librepaper` remote. Get a grant through "Consent on first use"
   if this computer has none for the project.
4. Write `.librepaper` if it is missing, and commit it alone with the
   message `Connect to LibrePaper project <slug>`.
5. Download the project. List the paths that exist in `HEAD` but not in the
   project and are not already local-only. Ask once per path, or once for
   all of them, whether to add it to `.librepaperignore` (kept local) or
   upload it (pushed on the next push).
6. Write the first LibrePaper commit with `HEAD` as its parent, so the two
   histories are related from the start. Its tree is the project's files
   plus the local-only paths from `HEAD`.
7. Print a diffstat of `HEAD` against that commit and say: "Run `git merge
   librepaper/main` to take LibrePaper's version. To keep your local
   version instead, merge, then `git revert` the LibrePaper commit and
   push."

## Tests

Server, in the gated Postgres suite:

- a grant works for fetch at viewer role and for push only at editor role;
- a token is handed out once, only after approval, only with the right
  secret, and never after the request expires;
- approval refuses a person without access to the project;
- revoking, signing out everywhere and removing the person each end access;
- the push route applies all or nothing;
- a touched file changed since `base` refuses, while an untouched file
  changed since `base` does not;
- the git route's limits are enforced, and the agent limits are unchanged;
- a replayed operation key returns the first receipt;
- the stored mutation has `automation: false` and `source: "git"`.

Companion, unit tests with a temporary repository:

- clone, then pull with no change, makes no new commit;
- local-only paths survive fetch and are never pushed;
- CRLF blobs push as LF without changing every line;
- each refusal rule produces its message;
- hunk conflicts follow `git merge-file` on a shared fixture corpus;
- link joins histories, so the first `git merge` is not "unrelated";
- link with no argument reads `.librepaper`, and the hint appears when
  the file exists without a remote;
- the short URL resolves from `.librepaper`, from a single paired site, and
  refuses when ambiguous.

Browser, against the mock server:

- the consent page shows Allow only when the paired companion confirms the
  request id, and refuses a request it does not know;
- with no companion answering, the page asks for the typed code.

End to end, against a real `admin serve` (see the Node full-deployment
tests):

- two browsers typing in `paper.qmd` while a push edits other lines of the
  same file: the push applies, and both browsers converge;
- a comment anchored on untouched text keeps its anchor through a push;
- a push overlapping a browser edit is refused, then `git pull` shows
  conflict markers, and the resolved push applies;
- a binary figure push replaces the figure in the editor.

## Later

- **Connect from the browser.** "Clone to this computer" and "Connect a
  local repository" in the project menu. The companion opens a native
  folder dialog and runs `clone` or `link`. The question about files that
  exist only in the repository becomes a checklist in the page.
- **New project from a local repository.** Pick a folder, and a project is
  created from its files with the remote already set.
- **New project from GitHub.** The server fetches public repositories. For
  private ones, the companion clones with the user's own keys.
- **One grant per account per computer.** An opt-in choice on the consent
  page: "all projects I can edit, on this computer". It ends repeat
  consent, at the cost of a wider grant.
- **Push as tracked changes.** `git push -o review` sends
  `"publish": "suggestions"`, so coauthors review the change as tracked
  changes. The server already supports it through the shared core.
- **Named versions as tags.** Fetch writes a tag for each named version
  created since the last fetch.
- **Hosted access without the companion.** The same `/api/git/v1/` routes,
  behind a smart-HTTP front end on the server, would let CI and lab servers
  use plain `git` with the grant as a password. The push logic would move
  from the helper to the server unchanged.
- **Shared commit ids.** The server stores commit metadata per fetch and
  push so every machine writes the same commits.
- **Range-level checks in the shared core.** See "Improvements that serve
  both callers".
