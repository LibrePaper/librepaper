# SPEC: LibrePaper as a git remote

Status: proposed 2026-10-14, revised 2026-10-18, 2026-10-20 and
2026-10-21, not started. This replaces the earlier project-folder sync design in full. The
2026-10-18 revision moves the helper from the retired companion to the
`librepaper` command that every desktop app package installs, signs in with
`librepaper login` instead of a new git grant, and replaces the three-way
push check with a strict per-file hash check. The 2026-10-20 revision drops
the committed `.librepaper` and `.librepaperignore` files. The remote URL
names the project, and the local-only list lives under `.git/`. The
2026-10-21 revision makes `link` merge against a root commit, makes the
server's checks use the suggestion-free projection, and treats a file
LibrePaper cannot hold as local-only. A later 2026-10-21 revision makes the
remote URL a share link with `librepaper::` in front: the login token says
who the user is, and the link's key says what they may do, because the
server bounds every automation request by a presented link. A third
2026-10-21 revision drops every `librepaper git` command, so plain git
with the git URL does every job. Local-only paths are marked with a git
attribute, a first fetch adopts a LibrePaper commit already in the
repository's history, and a repository with its own history connects with
`git pull --allow-unrelated-histories`.

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

- The workflow is plain git. Given the git URL, `git clone`, `git pull`
  and `git push` do every job, and no LibrePaper command is part of it.
- The helper never acts as a Loro peer. It sends plain text patches, and
  the server turns them into document edits.
- The helper never merges. Git merges, during `git pull`.
- Concurrent edits to the same file are never merged by Loro. The server
  checks and applies in one step under its transaction lock, so there is
  no race to retry.
- No browser tab and no running desktop app are needed. The helper reads
  the token that `librepaper login` stored, and the share link's key from
  the remote URL.
- Git access is a share link. Its role decides fetch or push, and
  replacing or revoking the link ends git access with it.
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
   plain git commands.

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

The Share dialog gives each link (read, comment, edit) a "Copy git URL"
button next to "Copy link". The git URL is the share link with
`librepaper::` in front. Every workflow is plain git with that URL:

```sh
git clone "librepaper::https://librepaper.org/docs/<slug>#k=<key>" thesis
```

The quotes keep the shell from reading `#` as a comment. A repository
that already has commits connects with `git remote add` and a first pull,
see "Connecting an existing repository".

- **Remote names.** `librepaper` in `git pull librepaper main` is the name
  given to the remote by `git remote add`, in the same way GitHub is
  usually `origin`. A repository that also pushes to GitHub keeps `origin`
  for GitHub and `librepaper` for the project. A repository cloned from
  LibrePaper calls the remote `origin`, and plain `git pull` works.
- **How git finds the helper.** Git runs a program named
  `git-remote-<x>` for any URL of the form `<x>::<address>`, passing it
  the remote's name and the address unchanged. Git's own `https://`
  support works the same way, through `git-remote-https`, as do
  `git-remote-hg` and `git-remote-gcrypt`. Without a desktop app package
  that installs the helper, git stops with `fatal: Unable to find remote
  helper for 'librepaper'`.
- **Sign-in on first use.** When the helper finds no stored token for the
  site, it runs the `librepaper login` device flow in the terminal: it
  prints a URL and a code, waits for approval in the browser, stores the
  token and continues the git command. Git owns the helper's standard
  input and output, so the prompt goes to standard error and the helper
  checks for a terminal on `/dev/tty` (the console on Windows). With no
  terminal, for example a clone started from an editor, the helper refuses
  at once and says to run `librepaper login`. See "Credentials".
- **The URL holds the key.** Git keeps the remote URL in `.git/config`,
  which is never committed or pushed. `git remote -v` shows the key, like a
  GitHub token written into a URL. The docs say never to paste that URL
  into a README or an issue. The plain share link without `librepaper::`
  carries the same key. No commit ever holds a key.
- **The owner.** The owner uses an edit link like anyone else. When the
  project has none, the owner creates one in the Share dialog first, and
  the "Use with git" panel in the project menu says so and opens Share.

### Connecting an existing repository

A repository that already has commits connects with `git remote add` and
a first pull. What that pull needs depends on where the repository came
from.

| Starting point | First pull |
|---|---|
| A clone of a repository that already syncs with this project, for example a coauthor's clone from GitHub | `git pull librepaper main` |
| A repository and a project that were never connected | `git pull --allow-unrelated-histories librepaper main` |
| The same, with the repository winning where both sides changed the same lines | `git pull --allow-unrelated-histories -X ours librepaper main` |
| The same, with the project winning | `git pull --allow-unrelated-histories -X theirs librepaper main` |
| A repository going into a new, empty project | `git pull --allow-unrelated-histories librepaper main` |

Each starts with
`git remote add librepaper "librepaper::https://<site>/docs/<slug>#k=<key>"`.
After the first pull, the user resolves any conflicts, commits, marks
local-only paths (see "Local-only paths") and pushes. Every later pull and
push is plain.

- **Already connected.** Fetch commits carry a `LibrePaper-Project`
  trailer. A first fetch adopts the newest such commit in `HEAD`'s history
  as base, so the pull is an ordinary three-way merge and needs no flag.
  See "First fetch".
- **Never connected.** The repository and the project share no version.
  The first fetch writes a root commit, and the merge uses an empty base:
  a path on one side only is kept, identical paths merge cleanly, and a
  path that differs is an add/add conflict. Any shared version git
  assumed instead would be a lie that loses work without a conflict. Git
  refuses such a merge without `--allow-unrelated-histories`, so nothing
  merges until the user chooses. The helper prints the command on
  standard error before git's refusal.
- **Choosing a side.** `-X ours` and `-X theirs` pick a side only for
  lines both sides changed. Paths on one side only are kept either way. A
  file where both sides made unrelated edits keeps both sets of edits, so
  the docs recommend the plain merge when in doubt.
- **An empty project.** Its first fetch is a root commit with an empty
  tree, so the merge adds the repository's files and the push uploads
  them. It needs no special case.
- **No force.** `git push --force` is refused, see "Push". A user who wants
  the repository to win uses `-X ours` on the first pull, which deletes
  nothing in the project.

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
   uses with the login token and a link key. Push gets one thin route,
   `POST /api/git/v1/{slug}/push`, on one source-change core shared with
   the agent tools. See "Server apply".

The helper speaks git's remote-helper protocol with the `import` and
`export` capabilities. Git produces and consumes fast-import and fast-export
streams, so the helper never writes pack files or computes object ids by
hand. It reads blobs from the user's repository with `git cat-file`. It
needs no merge code, because a push either matches the server exactly in
every touched file or is refused.

## Credentials

The helper sends two things, as `librepaper export` and `librepaper agent`
already do (`auth_headers` in
`crates/librepaper-companion/src/automation/peer.rs`). It adds no new
credential type.

- **Who: the login token.** The device-flow token (`lp_` prefix), sent as
  a bearer. It is stored in the token cache under the state home, keyed by
  the server's origin, with the file lock in
  `crates/librepaper-companion/src/local/credentials.rs`. A token for one
  deployment is never sent to another.
- **What: the link key.** The `k` from the remote URL's fragment, sent in
  the `x-librepaper-key` header, never in a query string. The helper
  marks its requests as automation like the other CLI commands.
- **Why both.** The server bounds every automation request by a presented
  link: `may_read` in `crates/librepaper-server/src/server/mod.rs` refuses
  automation without a live link, even for the owner, and does not fall
  back on the guest pin a browser visit left. A token alone opens nothing.
  Every link needs a signed-in account, so the key alone opens nothing
  either.
- **Role.** Every request resolves the role from the link, capped by the
  account's own ceiling, as for any link holder. A read or comment link
  can fetch. Only an edit link can push.
- **Revocation.** Replacing or revoking the link, or its expiry, ends git
  access at the next request. A device token ends with the session that
  approved it, and signing out everywhere revokes it.
- **Refusals.** A refused token makes the helper say to run `librepaper
  login`. A dead link makes it say to ask for a new link and run `git
  remote set-url <remote> "<new git URL>"`. The helper never starts a
  sign-in in the middle of a push.
- **Scope.** A token and a key together reach one project, at the link's
  role. The key in `.git/config` is the main secret on disk, and it is
  the same secret the browser keeps for that link.

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

## Remote URL forms

There is one form:

```text
librepaper::https://<site>/docs/<slug>#k=<key>
```

It is a share link (`/docs/{slug}#k={key}`, as `sharing.rs` writes it)
with `librepaper::` in front. The site picks the stored token, and the
fragment is the link key. A URL without `#k=` refuses and says to copy the
git URL from the Share dialog. No file in the repository names the
project. Fetch commits name it in a trailer without the key, see "First
fetch".

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
bytes. The server computes the same thing with `digest_of`
(`crates/librepaper-engine/src/storage/store.rs`), applied to the file's
projection without pending suggestions (see "Pending tracked changes are
not fetched"), not to the stored text. For a binary file the hash is the
asset hash, which is the SHA-256 of its bytes. Git's own object ids hash a
header plus the content and never equal these hashes, so the helper never
reuses them.

## Fetch

### Server read

`GET /api/documents/{slug}/project` returns `schema`, `project_digest`,
`tree` and `texts`. It accepts the login token and link key, in the same
way `/snapshot` does for `librepaper export`, and supports `If-None-Match`.
Binary files are read from `GET /api/documents/{slug}/assets/{sha}`, which
`librepaper export` already reads with the same pair.

`project_digest` covers every path, the projected text of every text file
and the asset hash of every binary file. Renaming a file or replacing a
figure changes it. Comments and pending tracked-change records do not, so
adding a comment never makes a fetch write a commit or a push refuse.

### Text or binary

A path's extension decides whether it is text or binary, by the
deployment's `text_extensions` and `asset_extensions` (`kind_of` in the
projection's path rules). The helper reads both lists from `/api/config`,
as the web app does, so it never keeps its own copy. A path whose extension
is in neither list cannot be a project file.

### Pending tracked changes are not fetched

Tracked changes live in the document (`changes` root records), so a file
with pending suggestions holds more than one plausible text. Fetch writes
the text without them: suggested insertions are left out and suggested
deletions are kept. Suggestions stay in LibrePaper, to be accepted or
rejected there.

- The git routes read, hash and digest this suggestion-free projection.
  `texts`, `project_digest` and the per-file dependency hashes all use it,
  so a new suggestion never makes a fetch write a commit or a push refuse.
- A push's patch offsets are offsets in that projection. The server maps
  them onto the document text, stepping over suggested insertions.
- A patch that overlaps or touches a pending suggestion refuses, naming the
  file and the suggestion. Suggestions elsewhere in the file are left
  alone.
- Accepting a suggestion in LibrePaper changes the projection, so the next
  fetch carries it as an ordinary edit.

### Steps

1. Read `state.json`. If the server's `project_digest` equals the recorded
   one, report the recorded commit as `main` and stop. Nothing changed.
2. Download the project. Download each binary whose content hash is not
   already a blob in the repository.
3. Build the new tree:
   - every project file, at its project path, with mode `100644`;
   - every path in base's tree that is local-only (see below), unchanged.
4. Write one commit through fast-import:
   - parent: base, or as "First fetch" says when there is no base;
   - author and committer: `LibrePaper <git@librepaper.org>`, dated at the
     fetch time;
   - message: `LibrePaper edits` plus a blank line and two trailers,
     `LibrePaper-Project: https://<site>/docs/<slug>` and
     `LibrePaper-Digest: <project_digest>`.
5. Move base: update the ref and `state.json`.

### First fetch

With no base, the helper decides the new commit's parent from the
repository:

1. When `HEAD` is unborn, as in `git clone` or a fresh `git init`, the
   commit has no parent.
2. Otherwise the helper looks for the newest commit in `HEAD`'s history
   whose `LibrePaper-Project` trailer names this project. When it finds
   one, that commit becomes base, with `project_digest` set to `null`, and
   fetch continues from step 2 as usual. The commit's tree holds the
   project as it was when that commit was written, so the pull is a
   three-way merge against a version both sides really share.
3. Otherwise the commit has no parent, and the helper writes to standard
   error: "This repository has its own history. To merge it with the
   project, run: git pull --allow-unrelated-histories librepaper main. See
   https://<site>/docs/git." Git then refuses a plain merge with its own
   message.

The trailer is the share link without the key. A slug opens nothing
without a key, so a public repository can carry it. A trailer that was
edited or forged can only make a push refuse, because the server still
checks every touched file's hash.

### Local-only paths

A git repository often holds files the project should not, such as a
Makefile, data or CI configuration. Fetch must not delete them, and push
must not upload them.

- A path is marked local-only by unsetting the `librepaper` git attribute:

  ```text
  # .gitattributes: committed, shared by every clone
  data/**          -librepaper
  draft-notes.md   -librepaper
  ```

  `.git/info/attributes` takes the same lines for one clone only. A path
  committed to a shared repository is marked in `.gitattributes`, because
  a mark in one clone's `.git/info/attributes` is invisible to the
  others, which push the file.
- The helper reads attributes from the commit it is working on, never
  from the working tree: base's tree on fetch, the pushed commit's tree on
  push, plus `.git/info/attributes`. `git check-attr --source` does this
  from git 2.40. With an older git, the helper reads the `.gitattributes`
  blobs itself.
- These paths are always local-only and need no mark: `.git/`,
  `.gitignore`, `.gitattributes`, `.github/`, and `*.local`.
- A path whose extension is in neither `text_extensions` nor
  `asset_extensions` is local-only too, because LibrePaper cannot hold it.
  A `Makefile` or an `.Rproj` file needs no mark.
- Fetch carries local-only paths over from base unchanged.
- Push skips them.

Marks cannot be inferred from the trees. A path in base that is missing
from the project could be local-only or could have been deleted in
LibrePaper, and only the attribute tells them apart. Because
`.gitattributes` travels with the history, a coauthor whose first fetch
adopts another clone's commit reads the same marks from it.

A file added locally is pushed like any other change, with no extra step,
unless it is local-only. A file a coauthor added to the shared repository
reaches LibrePaper on the next push from any clone, unless it is marked.
That is the same as the coauthor pushing it, and marking it and deleting it
in LibrePaper undoes it.

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
2. **Ancestry check.** With no base, refuse and say to pull first. The
   pushed commit must descend from base. Otherwise refuse, even for
   `--force`, which git passes to the helper as `option force true`.
   LibrePaper never rewinds. A plain `git fetch` moves base without
   merging it, so the message says to merge rather than to fetch:
   "LibrePaper has edits you have not merged. Run: git pull librepaper
   main, then push again." When base is a root commit, the message names
   `git pull --allow-unrelated-histories librepaper main` instead.
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
6. **Dependencies.** For every touched text file, patched or deleted, add
   a dependency with the base hash (see "Per-file hashes are computed, not
   stored"). A touched binary file carries the asset hash it must still
   hold in `expect`, on a `put_binary` or a `delete`. A created file has no
   dependency. The server refuses a creation over an existing path.
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
| Pushed commit does not descend from base | helper | `git pull`, then push |
| A text file contains a line starting with `<<<<<<< `, `=======` or `>>>>>>> ` | helper | file and line |
| A text file is not valid UTF-8 | helper | file |
| A path is a symlink or a submodule | helper | path |
| A touched file changed in LibrePaper since base | server | every such file |
| A created path already exists in LibrePaper | server | path |
| A patch overlaps or touches a pending suggestion | server | file and suggestion |
| A path fails the server's path rules (`validate_path` in `agent.rs`) | server | path |
| A file is larger than the project's per-file limit | server | file and limit |
| The push exceeds the account's storage quota | server | quota |
| The push is larger than one update may be | server | the log quota |
| The memory budget cannot hold the request | server | "busy, push again later" |
| The link is not an edit link, or the account cannot edit | server | role |
| The token is refused | server | `librepaper login` |
| The link was replaced, revoked or expired | server | `git remote set-url` with a new git URL |
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

### What exists and is reused

- **`PatchRequest`.** Byte-range patches with the `exact` text currently
  there, an operation key and a base tree digest.
- **`Consistency::UnchangedDependencies`.** A dependency with a
  `file_hash` refuses when that file's SHA-256 differs. One dependency per
  touched file is the strict check a push needs, so no new consistency
  mode is required. Today it hashes the stored text and stops at the first
  mismatch. Git needs both changed, see "Checks against the projection"
  below.
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
   | `Git` | false | `git` | none of its own, see "Push size" |

   The agent limits move from the constants `MAX_PATCHES` and
   `MAX_PATCH_BYTES` into the origin. They keep a model's edits small and
   reviewable, and do not protect the server. The recorded
   source drives the editor notice ("pushed from git") and the activity
   record. Like the operation scope, `Origin` is kept out of the request
   digest, so it cannot alter a retry's identity.
4. **Whether base matched.** The outcome reports `base_matched`, true when
   `base_tree` equaled the projection's `project_digest` just before the
   apply. The helper needs it to know whether the server now equals the
   pushed commit.
5. **Checks against the projection.** This is the largest new piece of
   server code. Today every check reads the stored text, which holds
   pending suggested insertions, and the untracked-edit planner
   (`track::untracked_edits`) moves pending records around a patch. Git
   fetches the projection without suggestions (see "Pending tracked
   changes are not fetched"), so for `Origin::Git` the core checks against
   that projection instead:
   - a dependency's `file_hash` is compared with `digest_of` of the
     projected text;
   - patch and dependency offsets are offsets in the projected text, and
     the core maps them onto the stored text, stepping over suggested
     insertions, before the planner applies them;
   - a patch whose range overlaps or touches a pending suggestion refuses,
     naming the file and the suggestion;
   - `base_tree` is compared with the projection's digest.

   `Origin::Agent` keeps checking the stored text, as it does today. A
   file with no pending suggestion projects to its stored text, so both
   origins give the same answers for such a file.
6. **Every failure, not the first.** Dependency validation collects every
   touched file that changed and refuses once with the whole list, instead
   of returning at the first mismatch.
7. **Tracked changes take a `SourceChange`.** `AgentTrackedCommand` accepts
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
  token and an edit link's key, editor role required. The helper is deterministic code, so it
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

### Push size

A git push has no patch or byte limit of its own. The deployment's
existing bounds already cover it, and a fixed git size would be the kind of
per-document constant the 2026-09-23 cost cutover removed.

- **One update.** The applied change is one update, and one update may be
  as large as the document's log quota (`max_update_bytes` in
  `crates/librepaper-base/src/config/budget.rs`, 32 MiB by default). A
  larger push refuses and names the quota.
- **Memory.** The request body reserves four times its declared length from
  the memory budget, as every mutation does. When that does not fit, the
  route answers `busy` and the helper says to push again later.
- **Storage.** The owner's storage quota applies as it does to typing.
- **All or nothing.** A push is never split into several mutations to fit,
  because that would break the all-or-nothing apply.

Real pushes are far below these bounds. A thesis is one or two MiB of
source, and figures arrive through the asset route, outside the log. A
2 MiB document rebuilds in about 8 ms since the `import_batch` fix.
Validating thousands of byte-range patches is cheap, and their bytes are
already counted.

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
- **The key sits in `.git/config`.** Anyone who copies a clone's
  `.git/config` and is signed in to LibrePaper gets that link's access,
  exactly as if they had the share link. Replacing the link in the Share
  dialog ends it, and every clone then needs `git remote set-url`.
- **Replacing a link breaks every clone.** That is the point of replacing
  it, but coauthors who use git must each update their remote.


## Build order

Each step can be merged and shipped on its own.

1. **Server: read.**
   - `/api/documents/{slug}/project` accepts the login token with a link
     key, as `/snapshot` does;
   - the git projection without pending suggestions, its digest and file
     hashes;
   - check that the asset route does too, as `librepaper export` relies on.
2. **Helper: fetch.**
   - `git-remote-librepaper` installed by every package that installs
     `librepaper`;
   - remote-helper protocol with `import`;
   - base state, the `librepaper` attribute, line endings, file modes;
   - the git URL and sign-in on first use, refusing without a terminal;
   - text or binary from `/api/config`;
   - the first fetch: adopting a commit with this project's trailer, or a
     root commit with the `--allow-unrelated-histories` hint.
   At this point clone, pull and backup to GitHub work, starting from a
   share link with `librepaper::` typed in front.
3. **Server: shared source-change core.**
   - rename the agent apply path to `SourceChange` and move it to
     `room/source_change.rs`, with no behavior change for agents;
   - file operations, `Origin` with the agent limits and the recorded
     source, `base_matched`;
   - checks against the projection for `Origin::Git`, and dependency
     refusals that list every changed file;
   - tracked changes accept a `SourceChange`;
   - the thin push route and the version handshake.
4. **Helper: push.**
   - `export` capability, ancestry check, local refusal rules, patch
     generation, per-file dependencies, state update.
5. **Web.**
   - "Copy git URL" on each link in the Share dialog;
   - "Use with git" panel in the project menu with the commands, pointing
     to Share for the URL;
   - the "pushed from git" notice in the editor.
6. **Docs.** A `docs/git.md` page with the commands for each starting
   point, the `--allow-unrelated-histories`, `-X ours` and `-X theirs`
   flags, the `librepaper` attribute, and every refusal message.

## Tests

Server, in the gated Postgres suite:

- with the login token, a read or comment link can fetch and not push,
  and an edit link can do both;
- the token without a key, and a key without a token, are each refused,
  for the owner too;
- replacing, revoking or expiring the link ends access at the next
  request, and signing out everywhere does too;
- the push route applies all or nothing;
- a touched file changed since base refuses and names the file, while an
  untouched file changed since base does not refuse;
- a refusal lists every changed touched file, not only the first;
- a file with a pending suggestion: its dependency hash is the projected
  text's, and an agent patch to the same file still checks the stored text;
- replacing a figure and renaming a file each change `project_digest`;
- deleting a binary refuses when its asset hash is not the expected one;
- `base_matched` is true only when the whole project equaled base;
- a creation over an existing path refuses, and a binary replacement
  refuses when the current asset is not the expected one;
- a git push larger than the agent limits applies, the agent limits are
  unchanged, and a push larger than the log quota refuses;
- a replayed operation key returns the first receipt;
- the stored mutation has `automation: false` and `source: "git"`;
- adding a comment does not change `project_digest`;
- fetched text leaves out pending suggestions, and a new suggestion does
  not change `project_digest`;
- a patch next to a suggestion lands at the right offset, and a patch
  that overlaps or touches one refuses and names it.

Helper, unit tests with a temporary repository:

- clone, then pull with no change, makes no new commit;
- local-only paths survive fetch and are never pushed;
- CRLF blobs push as LF without changing every line, and hash equal to
  the LF text;
- the per-file hash the helper computes equals the server's projected
  hash on a shared fixture, with and without pending suggestions;
- text or binary follows the lists from `/api/config`;
- a file added locally is pushed with no extra step;
- a path with an extension LibrePaper cannot hold is never pushed and
  survives every fetch, with no mark;
- a path marked `-librepaper` in `.gitattributes` or
  `.git/info/attributes` is never pushed and survives every fetch, and the
  marks are read from the commit, not the working tree;
- with no terminal, a first use refuses at once and names `librepaper
  login`;
- after a plain `git fetch`, pushing an unmerged `main` refuses with the
  merge message;
- each helper-side refusal rule produces its message;
- base moves on fetch and on an applied push, and not on a refusal;
- losing `state.json` is recovered from the ref;
- a first fetch with an unborn `HEAD` writes a root commit, and a first
  fetch into a history holding this project's trailer adopts the newest
  such commit as base, ignoring trailers for other projects;
- a first fetch into an unrelated history writes a root commit holding
  only the project files and prints the `--allow-unrelated-histories`
  hint, and the merge with that flag keeps paths only in `HEAD`, adds
  paths only in the project, merges identical files cleanly and conflicts
  on differing ones;
- that merge on a `HEAD` ahead of the project keeps those edits as
  conflicts, never reverting them, and a later merge with that same
  history reverts nothing;
- a coauthor's clone that adopts another clone's commit keeps the files
  that `.gitattributes` marks, and a pull after adoption merges three
  ways;
- a push with no base refuses and says to pull, a push after a plain
  fetch of a root commit names `--allow-unrelated-histories`, and `git
  push --force` refuses;
- the git URL yields the site and key, the key goes in the header and
  never in a query string, no commit holds the key, and a URL without
  `#k=` refuses;
- a dead link prints the `git remote set-url` advice.

End to end, against a real `admin serve` (see the Node full-deployment
tests):

- a browser typing in `intro.tex` while a push edits `paper.qmd`: the push
  applies, and the browser converges;
- a browser edit in `paper.qmd` after the last pull refuses a push to
  `paper.qmd`, then `git pull` merges (with conflict markers when the
  lines overlap), and the next push applies;
- a comment anchored on untouched text keeps its anchor through a push;
- a binary figure push replaces the figure in the editor;
- with the default quota, `git pull --allow-unrelated-histories` from an
  empty project then a push of a thesis-sized repository applies, and a push over the quota refuses with
  the quota message and leaves the project unchanged.

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
- **The login token alone, with git treated as the person** (the
  2026-10-18 to 2026-10-21 drafts). The helper would send no link key and
  the server would resolve the role from ownership and the guest pin a
  browser visit left. It weakens the rule that a local tool never inherits
  a browser's access and is always bounded by a presented link, and a
  coauthor would have to open the link in a browser before git worked.
- **A `librepaper git link` command** (the 2026-10-14 to 2026-10-21
  drafts), with `librepaper git clone` and `librepaper git local` beside
  it. `link` added the remote, asked once per path which files stay local,
  wrote a root commit and ran the unrelated-histories merge. Plain git
  does the same with one documented flag, and the per-path question became
  an attribute that travels with the repository. An earlier version of
  `link` gave the first LibrePaper commit `HEAD` as its parent. Then
  `git merge` fast-forwarded to the project's version, silently dropping
  files meant for upload, and when `HEAD` held a coauthor's GitHub commits
  not yet pushed to LibrePaper, a later merge reverted them without a
  conflict.
- **A local-only list under `.git/`** (the 2026-10-20 and 2026-10-21
  drafts). Each clone kept its own list, so a coauthor who cloned from
  GitHub had none, and a first fetch that adopted another clone's commit
  would read that clone's local-only files as deleted in LibrePaper.
- **Replacing a project with `git push --force`.** It would overwrite
  coauthors' browser edits made since the last pull. `-X ours` on the
  first pull gives the repository precedence without deleting anything.

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
  dialog and runs `git clone`, or `git remote add` and the first pull. A
  checklist of the files that exist only in the repository writes their
  marks to `.gitattributes`.
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
  plain `git` with a token and link key as the password. The push logic would move
  from the helper to the server unchanged.
- **Shared commit ids.** The server stores commit metadata per fetch and
  push so every machine writes the same commits.
