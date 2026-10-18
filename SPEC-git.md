# SPEC: LibrePaper as a git remote

Status: proposed 2026-10-14, revised 2026-10-18, not started. This replaces
the earlier project-folder sync design in full. The 2026-10-18 revision
moves the helper from the retired companion to the `librepaper` command
that every desktop app package installs, signs in with `librepaper login`
instead of a new git grant, and replaces the three-way push check with a
strict per-file hash check.

## Summary

Every desktop app package ships a git remote helper,
`git-remote-librepaper`. A user adds a LibrePaper project as a remote and
uses plain `git pull` and `git push`. Fetch turns the project's current
files into a commit. Push turns the pushed commits into minimal text edits
and applies them on the server.

A push applies only when every file it touches is still exactly what the
user last pulled. Otherwise the server refuses it, the user runs
`git pull`, git merges on the user's machine with ordinary conflict
markers, and the user pushes again. This is the rule GitHub applies to a
non-fast-forward push, narrowed to the touched files.

The design is conservative on purpose:

- The helper never acts as a Loro peer. It sends plain text patches, and
  the server turns them into document edits.
- The helper never merges. Git merges, during `git pull`.
- Concurrent edits to the same file are never merged by Loro. The server
  checks and applies in one step under its transaction lock, so there is
  no race to retry.
- No browser tab and no running desktop app are needed. The helper reads
  the token that `librepaper login` stored.
- The server holds no third-party git credential and builds no git objects.

## Goals

1. `git clone`, `git pull` and `git push` against a LibrePaper project, from
   any desktop where the desktop app is installed.
2. A push lands in open editors live, like a coauthor's typing.
3. Comments and pending tracked changes stay attached to text that a push
   does not touch.
4. A push is refused only when a file it touches changed in LibrePaper
   since the last pull, never because someone typed in another file.
5. Every refusal names the file and the reason, and says what to run next.
6. An existing git repository can be connected to an existing project with
   one command.

## Non-goals for v1

- Use without the desktop app installed, for example from CI or a lab
  server. See "Later".
- Commit ids that match across machines. See "Known limits".
- Named versions as commits or tags.
- More than one branch. The remote has exactly one branch, `main`.
- Git LFS, submodules and symlinks.
- Accepting a push when someone else edited the same file elsewhere. See
  "Range-level checks" under "Later".
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

- **Remote names.** `librepaper` in `git pull librepaper main` is the name
  given to the remote by `git remote add`, in the same way GitHub is
  usually `origin`. A repository that also pushes to GitHub keeps `origin`
  for GitHub and `librepaper` for the project. A repository cloned from
  LibrePaper calls the remote `origin`, and plain `git pull` works.
- **Why the URL looks unusual.** Git runs a program named
  `git-remote-<x>` for any URL of the form `<x>::<address>`. Other tools
  use the same mechanism, for example `git-remote-hg` and
  `git-remote-gcrypt`.
- **Sign-in on first use.** When the helper finds no stored token for the
  site, it runs the `librepaper login` device flow in the terminal: it
  prints a URL and a code, waits for approval in the browser, stores the
  token and continues the git command. See "Credentials".
- **Short URLs.** `librepaper::<slug>` means the project `<slug>` on the
  site named in `.librepaper`, or else the only site the user is signed in
  to. The full form `librepaper::https://librepaper.org/p/<slug>` names the
  site explicitly. Both forms work everywhere a remote URL is accepted.
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
  with the slug filled in.

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
  paper.qmd was edited in LibrePaper since your last pull.
  Run: git pull librepaper main, then push again.
```

The full list is in "Push: refusal rules".

## Architecture

There are two parts.

1. **The helper.** `git-remote-librepaper` is the `librepaper` binary under
   a second name. Every desktop app package that puts `librepaper` on PATH
   puts the helper next to it: the Homebrew cask, Scoop, the Windows
   setup.exe, the deb and rpm packages and the Nix flake. The plain macOS
   .dmg puts nothing on PATH, so Mac users who want git use the cask. Git
   starts the helper whenever a remote URL begins with `librepaper::`. It
   runs as a short-lived process for one fetch or push. It reads the token
   store directly, so neither the desktop app nor its local service has to
   be running.
2. **The server.** Fetch uses the routes that `librepaper export` already
   uses with the login token. Push gets one thin route,
   `POST /api/git/v1/{slug}/push`, on one source-change core shared with
   the agent tools. See "Server apply".

The helper speaks git's remote-helper protocol with the `import` and
`export` capabilities. Git produces and consumes fast-import and fast-export
streams, so the helper never writes pack files or computes object ids by
hand. It reads blobs from the user's repository with `git cat-file`. It
needs no merge code, because a push either matches the server exactly in
every touched file or is refused.

## Credentials

The helper uses the token that `librepaper login` stores. It adds no new
credential type.

- **What it is.** The device-flow token (`lp_` prefix) that the CLI already
  sends as a bearer to `/api/me`, `/api/documents/{slug}/snapshot` and the
  asset routes (`crates/librepaper/src/cli/tokens.rs`).
- **Storage.** The token cache under the state home, keyed by the server's
  origin, with the file lock in
  `crates/librepaper-companion/src/local/credentials.rs`. A token for one
  deployment is never sent to another.
- **Role.** Every request rechecks the account's current role on the
  document. Viewers and commenters can fetch. Only editors can push.
  Removing a person from the project ends their git access at once.
- **Lifetime and revocation.** A device token ends with the session that
  approved it, and signing out everywhere revokes it. When the token is
  refused, the helper says to run `librepaper login` and stops. It does not
  start a sign-in in the middle of a push.
- **Scope.** The token covers the whole account, not one project. This is
  wider than the per-project grant in the 2026-10-14 draft. The `agent`
  command already accepts the same token, and a per-project grant needed a
  new table, routes, a consent page and a phishing guard. See "Rejected
  designs".

### Version handshake

Every helper request sends `LibrePaper-Git: 1`. The git routes answer
`426 Upgrade Required` when the server no longer accepts that version, with
a body naming the minimum version and the update command for each install
method. The helper prints the message as the git error. The same header
lets the server log which helper versions are still in use.

The helper never updates itself. It updates with the desktop app:

| Install method | Update command |
|---|---|
| Homebrew cask | `brew upgrade librepaper` |
| Scoop | `scoop update librepaper` |
| Windows setup.exe, macOS app | the app's own updater |
| deb, rpm | the system package manager |
| Nix | update the flake input |

The helper detects the method from its own install path.

## The `.librepaper` file

```toml
# Written by librepaper. Names the LibrePaper project this repository syncs with.
site = "https://librepaper.org"
project = "<slug>"
```

- It holds no secret, so committing it is safe.
- It is local-only: it is never uploaded into the project.
- `site` lets `librepaper::<slug>` resolve correctly for a coauthor who is
  signed in to more than one site.
- A repository with two LibrePaper remotes keeps the first one in the file.
  The second remote works, but `link` with no argument uses the file.

## Remote URL forms

| Form | Meaning |
|---|---|
| `librepaper::<slug>` | Project `<slug>` on the site named in `.librepaper`, or else the only site with a stored token |
| `librepaper::https://<site>/p/<slug>` | Project `<slug>` on `<site>` |

When the short form is ambiguous because the user is signed in to several
sites and no `.librepaper` names one, the helper refuses and lists the full
URLs to choose from. When the user is signed in nowhere, the helper uses
the default server, as `librepaper login` does.

## Base: the state kept by the helper

**Base** is the last commit exchanged with LibrePaper. It is the parent of
every fetch commit and the reference point of every push. It is an
ordinary commit in the user's repository, and git never sends it anywhere.

### Where it is stored

- `refs/librepaper/<remote>/main` points at base. Being a ref keeps the
  commit safe from garbage collection, and the user can inspect it with
  `git log`.
- `.git/librepaper/<remote>/state.json` holds:
  - `commit`, the same commit id as the ref;
  - `project_digest`, the server's projection digest at that commit, or
    `null` when unknown;
  - `marks`, the path of the fast-import and fast-export marks file.

Losing `state.json` loses nothing that matters. The helper rebuilds it from
the ref and learns the digest on the next fetch.

### When it moves

Base moves in exactly two cases:

1. **Fetch.** The new LibrePaper commit becomes base, and `project_digest`
   is set to the digest the server reported.
2. **A push that applies.** The pushed commit becomes base. `project_digest`
   is set to the server's new digest only when the server's whole project
   equaled base before the push. Otherwise it is set to `null`, so the next
   fetch makes a commit carrying the edits others made in untouched files.

A refused push leaves base where it was.

### Per-file hashes are computed, not stored

A push names, for each touched file, the hash the server must still hold.
The helper computes it from base on the spot: it reads the file's blob
from base's tree, normalizes CRLF to LF, and takes SHA-256 of the UTF-8
bytes. That is what the server's `digest_of`
(`crates/librepaper-engine/src/storage/store.rs`) computes. Git's own
object ids hash a header plus the content and never equal these hashes, so
the helper never reuses them.

## Fetch

### Server read

`GET /api/documents/{slug}/project` returns `schema`, `project_digest`,
`tree` and `texts`. It accepts the login token as a bearer, in the same way
`/snapshot` does for `librepaper export`, and supports `If-None-Match`.
Binary files are read from `GET /api/documents/{slug}/assets/{sha}`, which
`librepaper export` already reads with the token.

`project_digest` covers file text only. Comments and pending tracked-change
records do not change it, so adding a comment never makes a fetch write a
commit or a push refuse.

### Steps

1. Read `state.json`. If the server's `project_digest` equals the recorded
   one, report the recorded commit as `main` and stop. Nothing changed.
2. Download the project. Download each binary whose content hash is not
   already a blob in the repository.
3. Build the new tree:
   - every project file, at its project path, with mode `100644`;
   - every path in base's tree that is local-only (see below), unchanged.
4. Write one commit through fast-import:
   - parent: base, or none on the first clone;
   - author and committer: `LibrePaper <git@librepaper.org>`, dated at the
     fetch time;
   - message: `LibrePaper edits` plus a blank line and a trailer
     `LibrePaper-Digest: <project_digest>`.
5. Move base: update the ref and `state.json`.

### Local-only paths

A git repository often holds files the project should not, such as a
Makefile, data or CI configuration. Fetch must not delete them, and push
must not upload them.

- `.librepaperignore` at the repository root uses gitignore syntax.
- These paths are always local-only: `.git/`, `.gitignore`,
  `.gitattributes`, `.librepaper`, `.librepaperignore`, `.github/`, and
  `*.local`.
- Fetch carries local-only paths over from base unchanged.
- Push skips them.

### Line endings

Project text uses LF. Fetch writes LF. With `core.autocrlf` set, git
converts files in the working copy and converts them back on commit, so
committed blobs stay LF. Push converts any CRLF in a text blob to LF before
hashing and diffing, so a Windows checkout never shows up as rewriting
every line or as a changed file.

### File modes

The project has no file modes. Fetch writes every file as `100644`. Push
ignores the executable bit. A symlink or a submodule in a pushed tree
refuses, naming the path.

## Push

### Inputs

Git sends a fast-export stream for `refs/heads/main`. The helper reads two
versions of every file:

- **base**: the file in base's tree, which is what the user last received
  from LibrePaper;
- **ours**: the file in the pushed commit's tree.

The helper does not read the server's current project. The server checks
it.

### Steps

1. **Branch check.** Refuse any ref other than `refs/heads/main`. Refuse
   ref deletion.
2. **Ancestry check.** The pushed commit must descend from base.
   Otherwise refuse with "fetch first", even for `--force`. LibrePaper
   never rewinds.
3. **Collect changes.** Diff base against ours, skipping local-only paths.
   Intermediate commits are collapsed. Only the net change is applied.
4. **Validate.** Apply every rule in "Push: refusal rules" that the helper
   can check locally. Any failure refuses the whole push.
5. **Build patches.** For each changed text file, compute line hunks from
   base to ours. Inside each hunk, compute a character diff and emit one
   patch per changed run, with byte offsets in base and the `exact` text
   there. Minimal patches are what keep comment anchors attached. Because
   the server applies only when the file still equals base, offsets in base
   are offsets in the server's file.
6. **Dependencies.** For every touched file, add a dependency with the
   base hash (see "Per-file hashes are computed, not stored"). A created
   file has no dependency. The server refuses a creation over an existing
   path.
7. **Submit.** Send one change set (see "Server apply") with `base_tree`
   set to base's `project_digest`.
8. **Record.** On success, move base to the pushed commit and set
   `project_digest` as described in "When it moves", using the server's
   `base_matched` answer. Report `ok refs/heads/main`. On a refusal, leave
   base alone and report `error refs/heads/main <reason>`.

There is no retry loop. The server checks and applies in one step, so a
refusal always means a touched file really changed, and the answer is
always `git pull`.

### Push: refusal rules

| Rule | Checked by | Message names |
|---|---|---|
| Ref is not `main`, or is a deletion | helper | the ref |
| Pushed commit does not descend from base | helper | "fetch first" |
| A text file contains a line starting with `<<<<<<< `, `=======` or `>>>>>>> ` | helper | file and line |
| A text file is not valid UTF-8 | helper | file |
| A path is a symlink or a submodule | helper | path |
| A touched file changed in LibrePaper since base | server | every such file |
| A created path already exists in LibrePaper | server | path |
| A path fails the server's path rules (`validate_path` in `agent.rs`) | server | path |
| A file is larger than the project's per-file limit | server | file and limit |
| The push exceeds the account's storage quota | server | quota |
| The push exceeds the git patch limits | server | the limit |
| The user is not an editor | server | role |
| The token is refused | server | `librepaper login` |
| The helper is too old | server | the update command |

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
  `file_hash` refuses when that file's SHA-256 differs. One dependency per
  touched file is exactly the strict check a push needs. No new consistency
  mode is required.
- **Sequencing and replay.** `AgentPatchCommand` runs inside
  `Sequencer::command` under the transaction lock, so the dependency check
  and the apply are one step. Its replay check returns the first outcome
  for a retried operation id.
- **Authorization.** `authorize_document_mutation` and the recheck
  callback.
- **Tracked changes.** `document_propose` with `publish: "suggestions"`
  applies patches as tracked changes through `AgentTrackedCommand`.

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
   - `put_binary`: a path, the asset hash already uploaded through the
     existing asset route, and the asset hash the path must hold now, or
     none for a new path.
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
4. **Whether base matched.** The outcome reports `base_matched`, true when
   `base_tree` equaled the project digest just before the apply. The
   helper needs it to know whether the server now equals the pushed commit.
5. **Tracked changes take a `SourceChange`.** `AgentTrackedCommand` accepts
   the same type, so any caller that can apply a change can also publish it
   for review. File operations cannot be tracked changes yet. A change that
   has them and asks for review is refused with that reason.

### The callers

- **MCP `document_apply`.** Unchanged for the model. It loads its stored
  candidate, builds a `SourceChange` with `Origin::Agent`, and calls
  `apply_source_change`. Views, signed range ids and candidates stay,
  because they protect a model from inventing offsets. They are the agent's
  front end, not part of the core.
- **Git push.** `POST /api/git/v1/{slug}/push`, authenticated by the login
  token, editor role required. The helper is deterministic code, so it
  sends a `SourceChange` directly:

  ```json
  {
    "operation": {"epoch": "...", "id": "v2...."},
    "base_tree": "<base's project_digest, or empty when unknown>",
    "consistency": "unchanged_dependencies",
    "dependencies": [{"path": "paper.qmd", "file_hash": "<sha256 of base text>", "start": 0, "end": 0, "exact": ""}],
    "patches": [{"path": "paper.qmd", "start": 120, "end": 131,
                 "exact": "old words", "replacement": "new words"}],
    "create": [{"path": "notes.md", "text": "..."}],
    "delete": ["old.tex"],
    "put_binary": [{"path": "fig/plot.png", "sha": "<new asset sha>", "expect": "<base asset sha>"}],
    "message": "Chapter 2 draft",
    "publish": "apply"
  }
  ```

  The route parses the body, sets `Origin::Git`, and calls
  `apply_source_change`. With `"publish": "suggestions"` it calls the
  tracked-change command instead, which is how `git push -o review` will
  work. The route holds no logic of its own beyond authentication and
  parsing. `message` is the last pushed commit's subject, used for the
  notice.

The response is the new `project_digest`, `base_matched`, the applied
paths and the operation receipt. A dependency refusal lists every touched
file that changed, not only the first.

### On the helper side

The agent bridge client code already mints operation ids, retries with the
same id after a dropped connection, and maps refusals to messages. The
helper reuses it. A retry after a dropped connection resends the same
operation id, so it can never apply twice.

## Known limits

- **Any edit to a touched file refuses the push.** A coauthor who changed
  one line of `paper.qmd` in LibrePaper blocks a push to another part of
  `paper.qmd` until the user pulls. A coauthor typing continuously in that
  file can make a push fail several times in a row. Edits to other files
  never block. Range-level checks lift this. See "Later".
- **Commit ids differ across machines.** Each helper writes its own
  LibrePaper commits. Two people who both pull from LibrePaper and also
  share a GitHub repository get two commits for the same LibrePaper state.
  Git merges them cleanly, because both sides have the same content, but
  the history shows extra merge commits. Solo authors and people who share
  only through LibrePaper never see this.
- **Desktop only.** The helper ships with the desktop app packages.
- **History is coarse.** One LibrePaper commit per fetch that saw a
  change. The detailed history stays in LibrePaper.
- **The token is account-wide.** A stolen token reads and edits every
  project the account can edit, as it already can through `librepaper
  agent`.

## Open questions

These need answers before the step that depends on them.

- **Pending tracked changes in fetched text (before step 2).** Since
  tracked changes live in the document (`changes` root records), a file
  with pending suggestions has more than one plausible text: without the
  suggestions, with all of them accepted, or the raw document text. Fetch
  must pick one, and push must say what happens to a patch that edits
  inside a suggested insertion or deletion. The likely answer is: fetch
  writes the text without pending suggestions, and push refuses a patch
  that overlaps a pending suggestion, naming it.
- **Large first pushes (before step 4).** `librepaper git link` on an
  existing repository can push a whole thesis at once. Measure an 8 MiB,
  5,000-patch push against the per-document log quota and the compaction
  cost on a real `admin serve` before fixing the git limits.

## Build order

Each step can be merged and shipped on its own.

1. **Server: read.**
   - `/api/documents/{slug}/project` accepts the login token as a bearer;
   - check that the asset route does too, as `librepaper export` relies on.
2. **Helper: fetch.**
   - `git-remote-librepaper` installed by every package that installs
     `librepaper`;
   - remote-helper protocol with `import`;
   - base state, local-only paths, line endings, file modes;
   - both remote URL forms, and sign-in on first use;
   - `librepaper git clone` and `librepaper git link`, the `.librepaper`
     file, and the hint when a repository has the file but no remote.
   At this point clone, pull and backup to GitHub work.
3. **Server: shared source-change core.**
   - rename the agent apply path to `SourceChange` and move it to
     `room/source_change.rs`, with no behavior change for agents;
   - file operations, `Origin` with per-origin limits and the recorded
     source, `base_matched`;
   - tracked changes accept a `SourceChange`;
   - the thin push route and the version handshake.
4. **Helper: push.**
   - `export` capability, ancestry check, local refusal rules, patch
     generation, per-file dependencies, state update.
5. **Web.**
   - "Use with git" panel with the commands;
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
3. Add the `librepaper` remote. Sign in first if there is no token for the
   site.
4. Write `.librepaper` if it is missing, and commit it alone with the
   message `Connect to LibrePaper project <slug>`.
5. Download the project. List the paths that exist in `HEAD` but not in the
   project and are not already local-only. Ask once per path, or once for
   all of them, whether to add it to `.librepaperignore` (kept local) or
   upload it (pushed on the next push).
6. Write the first LibrePaper commit with `HEAD` as its parent, so the two
   histories are related from the start. Its tree is the project's files
   plus the local-only paths from `HEAD`. It becomes base.
7. Print a diffstat of `HEAD` against that commit and say: "Run `git merge
   librepaper/main` to take LibrePaper's version. To keep your local
   version instead, merge, then `git revert` the LibrePaper commit and
   push."

## Tests

Server, in the gated Postgres suite:

- the login token works for fetch at viewer role and for push only at
  editor role;
- signing out everywhere and removing the person each end access;
- the push route applies all or nothing;
- a touched file changed since base refuses and names the file, while an
  untouched file changed since base does not refuse;
- a refusal lists every changed touched file;
- `base_matched` is true only when the whole project equaled base;
- a creation over an existing path refuses, and a binary replacement
  refuses when the current asset is not the expected one;
- the git route's limits are enforced, and the agent limits are unchanged;
- a replayed operation key returns the first receipt;
- the stored mutation has `automation: false` and `source: "git"`;
- adding a comment does not change `project_digest`.

Helper, unit tests with a temporary repository:

- clone, then pull with no change, makes no new commit;
- local-only paths survive fetch and are never pushed;
- CRLF blobs push as LF without changing every line, and hash equal to
  the LF text;
- the per-file hash the helper computes equals the server's `digest_of`
  on a shared fixture;
- each helper-side refusal rule produces its message;
- base moves on fetch and on an applied push, and not on a refusal;
- losing `state.json` is recovered from the ref;
- link joins histories, so the first `git merge` is not "unrelated";
- link with no argument reads `.librepaper`, and the hint appears when
  the file exists without a remote;
- the short URL resolves from `.librepaper`, from a single signed-in site,
  and refuses when ambiguous.

End to end, against a real `admin serve` (see the Node full-deployment
tests):

- a browser typing in `intro.tex` while a push edits `paper.qmd`: the push
  applies, and the browser converges;
- a browser edit in `paper.qmd` after the last pull refuses a push to
  `paper.qmd`, then `git pull` merges (with conflict markers when the
  lines overlap), and the next push applies;
- a comment anchored on untouched text keeps its anchor through a push;
- a binary figure push replaces the figure in the editor.

## Rejected designs

- **Project-folder sync** (the design before 2026-10-14). Inbound folder
  sync with frontiers in the written record. Dropped for the remote helper.
- **A three-way check in the helper** (the 2026-10-14 draft). The helper
  read the server's current text, refused only overlapping line hunks, and
  mapped its patches onto the server's text, with a five-try retry when
  the file changed in between. It accepted more pushes than the strict
  check, but it reimplemented `git merge-file`, and its rule that touching
  hunks conflict would have refused identical changes on both sides, which
  git treats as clean. That case is common when coauthors share both
  GitHub and LibrePaper.
- **`git merge-tree` in the helper.** Letting the user's own git do the
  three-way merge (`git merge-tree --write-tree`, git 2.40 or later), then
  pushing the difference between the server's text and the merged result.
  It fixes the identical-change case and reuses git's rules, but it still
  needs a read of the server, a retry when the file changes before the
  apply, and Loro merging nothing only by luck of timing. The strict check
  needs none of that. It is the natural basis for accepting pushes over
  other edits later, if range-level checks are not enough.
- **A local bare mirror kept by the desktop app.** The user pushes to a
  plain file-path remote, and a `pre-receive` hook forwards the push to the
  server. It needs no remote-helper protocol, but git has no hook that runs
  before a fetch, so `git pull` returns whatever the mirror last synced.
  Stale pulls are the worst failure for this feature.
- **Pushing through the desktop app's open editor.** The helper hands the
  push to the app, and the project's editor page applies it as the user's
  own edits over the normal socket. It needs no server changes, but the
  app must be running with the project loaded, the hash check runs on a
  copy that can lag the server, so simultaneous edits get merged by Loro,
  and the service has no channel into a window today.
- **A per-project git grant with a consent page** (the 2026-10-14 draft).
  An `lpg_` token for one account, one project and one computer, minted by
  a browser consent page that asked the local companion to confirm the
  request, with a typed-code fallback over SSH. It was narrower than the
  login token, but it was the largest and riskiest new code in the design,
  and the companion it relied on is retired.

## Later

- **Range-level checks in the shared core.** Instead of refusing when a
  touched file changed at all, check each patch's range at the current
  head, by mapping its offsets from the base through the edits made since,
  with Loro cursors taken at the base and resolved at the head. It refuses
  only when the patched text itself changed. Text is still never merged by
  Loro: an edit inside a patched range is still a refusal. This lifts the
  busy-file limit for git and most "source tree changed" refusals for
  agents. It needs the server to know base's frontier, which a fetch would
  then record.
- **Connect from the desktop app.** "Clone to this computer" and "Connect
  a local repository" in the project menu. The app opens a native folder
  dialog and runs `clone` or `link`. The question about files that exist
  only in the repository becomes a checklist in the page.
- **New project from a local repository.** Pick a folder, and a project is
  created from its files with the remote already set.
- **New project from GitHub.** The server fetches public repositories. For
  private ones, the desktop app clones with the user's own keys.
- **Push as tracked changes.** `git push -o review` sends
  `"publish": "suggestions"`, so coauthors review the change as tracked
  changes. The server already supports it through the shared core.
- **Named versions as tags.** Fetch writes a tag for each named version
  created since the last fetch.
- **Hosted access without the desktop app.** The same routes, behind a
  smart-HTTP front end on the server, would let CI and lab servers use
  plain `git` with a token as the password. The push logic would move
  from the helper to the server unchanged.
- **Shared commit ids.** The server stores commit metadata per fetch and
  push so every machine writes the same commits.
