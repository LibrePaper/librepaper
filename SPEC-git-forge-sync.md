# SPEC: Sync a project with a GitHub repository

Status: proposed 2026-10-21, revised the same day after two reviews, not
started. This is the git work that goes ahead. SPEC-git.md, a remote
helper on every machine, and a hosted-remote variant drafted the same day
were both rejected; the first is kept on main with a rejection notice,
the second was never merged. What survives from them is in "Rules carried
over".

## Summary

A project can be connected to a branch of a GitHub repository, or to a
folder inside it. Pull brings the repository's changes into the live
document as a coauthor's edits. Push writes the project's files as a
commit on that branch. Files LibrePaper cannot hold are never touched.
Both are buttons. Nothing happens on its own.

The server stores no forge credential. The browser obtains a token
through GitHub's consent page and keeps it on the user's machine. The
browser talks to GitHub, computes what changed, and sends the server one
source change with per-file hashes and the sync bookkeeping that goes
with it. The server applies the change and records the bookkeeping in
one transaction, or refuses naming the files. The server's only
GitHub-facing part is a code-exchange relay that keeps nothing.

A pull that would collide with edits made in LibrePaper is refused
whole, and the project is left as it was. The user exports LibrePaper's
side as a branch, merges on GitHub, and pulls again. The merge happens
on GitHub or on the user's machine, never in LibrePaper and never by
Loro.

The first version is GitHub only and manual. GitLab, webhooks, automatic
pulls and pushing on a named version are in "Later", with the parts of
the design that they need.

### Rules carried over

Four rules from the rejected designs hold here unchanged:

- pull and push see the projection without pending suggestions, and a
  pull that lands on a pending suggestion refuses;
- a pull is applied as minimal character patches, so comment anchors on
  untouched text survive;
- one shared source-change core serves agents and sync, see "Server
  apply";
- a file changed on both sides is refused by name, never merged by Loro.

## Goals

1. Connect a project to a repository in one consent click, and pull or
   push in one more. Nothing to install, nothing to paste.
2. No forge credential on the server, at rest or in logs.
3. A pull lands in open editors live, like a coauthor's typing, and
   comments and pending tracked changes stay attached to text the pull
   does not touch.
4. A pull or push either happens entirely or not at all, and the record
   of what is synced is written in the same transaction as the change,
   so no crash, closed tab or second tab can leave the two apart.
5. Files the repository holds and LibrePaper cannot are never fetched,
   never pushed and never deleted.
6. A conflict is resolved with git's own tools, through a branch and a
   pull request, and the next pull finishes the job without reporting
   the same conflict twice.

## Non-goals for v1

- GitLab and other forges. See "Later".
- Anything automatic: webhooks, badges, pulling on open, pushing on a
  named version. See "Later".
- Partial pulls. A pull with any conflicting file applies nothing.
- Writing `.librepaperignore` for the user. An existing one is read.
- Importing the repository's commit history. Sync starts from the branch
  head.
- Branches other than the linked one, tags, Git LFS. A pointer file is
  skipped and listed, never written back.
- Merging inside LibrePaper.

## User experience

### Connecting

The project menu gets "Connect to GitHub". Any editor can connect.

1. A popup opens GitHub's consent page for the LibrePaper app, with its
   repository picker. The user approves and the popup closes.
2. The panel lists the repositories the user may write to. They pick
   one, a branch, and optionally a folder inside it. The folder is where
   the project's files live in the repository; everything outside it is
   ignored.
3. The panel shows the first comparison: files that will come in, files
   that will go out when the project is not empty, files that differ on
   both sides, and the top-level folders with how many files in each
   LibrePaper would sync. The user clicks "Pull" or "Push" to make the
   first sync, or "Cancel". With differing files, "Pull" is refused as
   described under "Conflicts", and "Push" overwrites the repository's
   copies, which the preview says in so many words.

After that the panel shows the repository, the branch, the folder, who
connected, when the last pull and push happened, "Pull", "Push", the
list of unsynced files, and "Disconnect".

### Daily use

- **Pull.** Fetches the branch head, compares with the last sync, and
  applies every change or none. The editor shows `<name> pulled 3 files
  from GitHub`, and coauthors see the text change where it changed.
- **Push.** Asks for a commit message, defaulting to the names of the
  changed files, and writes one commit on the branch. If the branch
  moved since the last sync, the push pulls first and then pushes; if
  that pull is refused, the push stops and the panel shows why.
- **Conflict.** The pull refuses, names the files, and offers "Create a
  branch with my changes". See "Conflicts".

### What is and is not synced

- A **synced file** is a path under the linked folder whose extension
  is in `text_extensions` or `asset_extensions`, not matched by the
  repository's `.librepaperignore`, not over the per-file limit, and not
  an LFS pointer.
- Everything else is **unsynced**: `.git*`, `.github/`, `Makefile`,
  `.Rproj`, `renv/`, `.rds`, `.parquet`, archives, binaries without a
  supported extension, anything outside the folder, anything the ignore
  file names. Unsynced files are never fetched, never written and never
  deleted. The panel lists them.
- Scripts and data in supported formats are project files. `.r`, `.py`,
  `.jl`, `.csv`, `.tsv`, `.json`, `.yml` and `.toml` are in the default
  text list because Quarto projects hold them. A `data/` folder of CSV
  files is synced unless the ignore file names it, and the preview's
  folder counts show that before the first sync.
- A supported file added on GitHub appears in LibrePaper at the next
  pull. A file deleted on GitHub is deleted in LibrePaper if it was
  synced. A file deleted in LibrePaper is deleted on GitHub if it was
  synced. A file LibrePaper never held is never deleted anywhere.
- Pending tracked changes are not pushed, and a pull that lands on one
  refuses. Suggestions are accepted or rejected in LibrePaper.
- Project text is LF. A CRLF file on GitHub is normalized on pull, and
  the next push writes LF, which shows once as a whole-file change on
  GitHub.
- Two repository paths that LibrePaper would consider the same, by case
  or Unicode normalization, cannot both be project files. A pull with
  such a pair refuses and names both. So does a path longer than
  `max_path`, and a repository with more syncable files than
  `max_files`.

### The ignore file

`.librepaperignore` is optional. It uses gitignore syntax, negation with
`!` included, and names paths LibrePaper must leave alone beyond the
built-in rules. The browser reads it from the repository root and, when
a folder is linked, from that folder too, and matches paths relative to
the linked folder with the `ignore` package, which implements the
gitignore rules. A matched path is unsynced in both directions, and a
synced file later added inside an ignored folder stays out. The file
cannot bring back what the built-in rules exclude, and the file itself,
having no supported extension, never enters the project. People write
it where they write `.gitignore`; the panel links to the docs page that
shows the two-line version.

### Conflicts

A conflict is a synced file that changed on GitHub since the last sync
and also changed in LibrePaper since the last sync, to different
content. A pull that finds one applies nothing and lists them. The
panel offers one action:

1. **"Create a branch with my changes."** LibrePaper writes a branch
   `librepaper/<date>` starting from the last synced commit, with one
   commit holding every synced file LibrePaper changed since that
   commit, conflicting or not. Starting from the synced commit is what
   lets git see LibrePaper's changes and GitHub's changes as two sides
   of one merge. The panel then shows GitHub's compare link for that
   branch against the connected branch, which opens the pull-request
   form.
2. The user merges on GitHub, resolving conflicts in its editor, or
   locally with git, and the connected branch gains the merge.
3. **The next pull finishes.** For each conflicting file, the pull
   checks that the project's copy still equals what was exported and
   that the branch head descends from the export. When both hold,
   LibrePaper's side is already in the merge, and GitHub's version
   applies over the project's copy. When someone kept typing in that
   file after the export, it is a conflict again, and the pull says so.
   A pull that consumes the export clears it.

One export at a time; a new one replaces the record and leaves the old
branch on GitHub. "Push" while an export is outstanding behaves as
always: the branch has moved, so it pulls first, and the pull either
finishes or refuses.

### Disconnecting and reconnecting

"Disconnect" forgets the link and the synced state. The repository is
untouched. If the connecting user's GitHub access ends, the next pull or
push fails with "Reconnect with your account", and any editor can
reconnect with their own consent; the link and the synced state are
kept.

## Architecture

```text
browser  <-- HTTPS, bearer token -->  api.github.com
   |
   |  one SourceChange, its sync record, an operation id
   v
server route  -->  source-change core and link row, one transaction
```

The browser does the GitHub work. The server does the document work and
keeps the record. The only server code that knows GitHub exists is the
code-exchange relay and the link table.

### Credentials, on the user's side

- `api.github.com` answers browser requests, measured 2026-10-21:
  `Access-Control-Allow-Origin: *` with the `Authorization` header
  allowed. GitHub's token endpoint does not, and GitHub requires the
  client secret even with PKCE. So the browser opens the consent page,
  and the server's relay exchanges the code with the secret and hands
  the token to the opener window with `postMessage`, checking the
  origin. The relay keeps nothing and logs nothing beyond the request
  line. Refresh goes through the same relay.
- The app is a GitHub App with "Contents: read and write" and
  "Metadata: read", installed on the repositories the user picks, with
  expiring user tokens on: eight hours, refreshed with a six-month
  refresh token. The pull-request form is reached through GitHub's
  compare link, so no "Pull requests" permission is asked for.
- The browser keeps the tokens in its local database for the signed-in
  account, next to the offline documents, and clears them at sign-out as
  it clears those. A token is never put in a URL. When a token is gone
  or refused, the panel offers the consent click again; for an
  already-authorized app GitHub returns at once with no prompt.
- Every call is made as the connected user. A commit's committer is that
  user; its author is set to the last LibrePaper editor as
  `Name <handle@users.noreply.<site>>`.
- The desktop app uses the same web flow. There is no separate desktop
  path.

### Durable state

One table, `forge_links`, one row per connected project:

- project id, provider, repository id and full name, branch, folder,
  connected by account;
- `synced_commit`, and `synced_map`: for every synced path, the raw
  GitHub blob id and the normalized content hash, SHA-256 of the LF
  text for a text file or the asset hash for a binary. The blob id
  detects a change on GitHub; the content hash detects a change in
  LibrePaper. They differ for every CRLF file, which is why both are
  kept;
- `export`, null or the branch name, the export commit, and the export
  map in the same shape as `synced_map`;
- `revision`, an integer the server increments on every write. Every
  write from the browser carries the revision it read; a mismatch
  answers 409 with the current row, and the browser plans again from
  it. Two tabs cannot record baselines out of order, and a tab that
  planned from a stale row cannot apply;
- last pull at, last push at.

Rows change on connect, pull, push, export and disconnect.

### Routes

| Route | Who | Does |
|---|---|---|
| `GET /api/forge/github/authorize` | editor | redirects to GitHub's consent page with state |
| `GET /api/forge/github/callback` | editor | exchanges the code, posts the token to the opener, stores nothing |
| `POST /api/forge/github/refresh` | editor | relays a refresh, stores nothing |
| `GET`, `PUT`, `DELETE /api/documents/{slug}/forge` | editor | read, write, clear the link; `PUT` carries the revision and is how connect, a push's record and an export are written |
| `POST /api/documents/{slug}/source-change` | editor | apply one `SourceChange`; with a `sync` block, write the link in the same transaction |

### The source-change route

The browser is an editor, so it may already change the document over
its socket. This route exists because a pull must be atomic and checked:
every touched file carries the SHA-256 of the projected text the browser
based its patches on, the core applies all of it or refuses with every
mismatched file, and the sync bookkeeping is written only if the apply
succeeds, in the same transaction.

- The body is a `SourceChange`, see "Server apply", with `Origin::Sync`
  and the provider name, which drives the notice and the activity
  record. Binaries are uploaded through the asset route first and
  referenced by hash in `put_binary`.
- `operation` is a fresh id the browser minted for this request and
  kept in its local database with the planned change until the server
  answered. A retry after a dropped connection resends the same id and
  gets the first receipt. A different request, even for the same GitHub
  commit, gets a different id. The GitHub commit is data in the `sync`
  block, never the id.
- `sync` holds the link revision the browser read, the new
  `synced_commit`, the new `synced_map`, and whether the export is
  consumed. The route checks the revision, applies the change, writes
  the row and increments the revision, all in one transaction. A
  revision mismatch answers 409 before anything is applied.

## Pull

In the browser, from the link row it just read:

1. `GET` the branch head. Equal to `synced_commit` means nothing to
   pull.
2. `GET` the recursive tree listing at the head. If GitHub marks it
   `truncated`, refuse: "the repository is too large to compare" and
   stop. The listing is the complete inventory; the compare endpoint,
   which caps its file list at 300, is not used.
3. When their blob ids changed since the sync, `GET` the ignore files.
   Filter the listing to synced paths under the folder through the
   built-in rules and the ignore rules. Note LFS pointers, oversize
   files, collision pairs and overlong paths. A collision pair or an
   overlong path refuses the pull by name.
4. For each path in the listing or in `synced_map`, decide:
   - **changed on GitHub** when the listing's blob id differs from the
     map's blob id, or the path is new, or the path is gone;
   - **changed in LibrePaper** when the project's normalized content
     hash differs from the map's content hash, or the project lacks a
     mapped path, or holds an unmapped one;
   - neither: nothing. GitHub only: fetch the blob and emit a patch set
     from the project text to the GitHub text with the character diff
     from `@codemirror/merge`, with the project text's hash as the
     dependency; a binary emits `put_binary` with `expect`; a new path
     emits `create` or `put_binary` without `expect`; a gone path emits
     `delete`. LibrePaper only: nothing, that is for push. Both, with
     identical content: nothing, and the map entry moves to the new blob
     id. Both, with different content: a conflict, unless the export
     rule under "Conflicts" says LibrePaper's side is already merged, in
     which case it is GitHub only.
5. Any conflict refuses the whole pull: nothing is sent, the panel lists
   the files and offers the export.
6. `POST` the source change with the `sync` block. On 409, read the row
   and start over. On a dependency refusal, which means someone typed in
   a touched file between steps 4 and 6, start over; after three tries
   show the file. On success, done.

A pull after a coauthor changed three files is two calls and three blob
fetches, plus the ignore files when they changed.

## Push

In the browser:

1. Read the link row. Compute every synced path's normalized content
   hash and compare with `synced_map`. Changed, added and deleted synced
   paths form the change set, after the ignore rules. An empty set is
   "nothing to push".
2. `GET` the branch head. If it is not `synced_commit`, run a pull and
   come back; if the pull refused, stop and show why.
3. `POST` a blob per changed file, `POST` a tree with `base_tree` set to
   the synced commit's tree and entries for the changed paths, deletions
   with a null sha, `POST` a commit with `synced_commit` as its parent,
   then `PATCH` the branch ref with `force: false`. A 422 means the
   branch moved after step 2; go to step 2 once more. The `PATCH` is a
   compare-and-swap on the parent, so the commit never lands on a head
   LibrePaper has not seen.
4. `PUT` the link with the new commit, the new map built from the blob
   ids GitHub returned and the content hashes the browser computed, and
   the revision. On 409, read the row: if its `synced_commit` is already
   the new commit, another tab recorded it; otherwise start over.

If the tab dies between steps 3 and 4, the next pull sees the branch
head at the pushed commit, finds every pushed file changed on both
sides with identical content, and records the baseline without
applying anything. Nothing is lost and nothing is applied twice.

The commit touches only synced paths. The tree is built on the synced
tree, so unsynced files carry over by construction.

## Export

The export is a push to a new ref:

1. Read the link row. The change set is every synced path changed in
   LibrePaper since `synced_commit`, computed as in push step 1.
2. `POST` blobs, a tree on the synced commit's tree, a commit with
   `synced_commit` as its parent and "LibrePaper changes" as the
   message, and `POST` a ref `refs/heads/librepaper/<date>`.
3. `PUT` the link with the export record, branch, commit and map, and
   the revision.
4. Show the compare link,
   `https://github.com/{owner}/{repo}/compare/{branch}...{export}`.

The export rule in pull step 4 is: a both-sides path is already merged
when the project's content hash equals the export map's content hash
for that path and a `GET compare {export}...{head}` reports the head as
`ahead` or `identical`. The second check is one call per pull while an
export is outstanding.

## Pending tracked changes

Tracked changes live in the document, so a file with pending suggestions
holds more than one plausible text. Sync uses the projection without
them: suggested insertions are left out and suggested deletions are kept.

- The browser's project text for every hash and every patch is that
  projection, which it computes from the document. Pushed text is that
  projection. Dependency hashes are of that projection.
- The core maps patch offsets onto the stored text, stepping over
  suggested insertions, before the untracked-edit planner applies them.
- A patch that overlaps or touches a pending suggestion refuses, naming
  the file and the suggestion. The pull reports it like a conflict:
  accept or reject the suggestion in LibrePaper, then pull again.
- Accepting a suggestion changes the projection, so the next push
  carries it as an ordinary edit.

## Server apply: one source-change core for agents and sync

Agents already turn a list of text patches into one document mutation,
with retries keyed by an operation id: `Room::apply_agent_request`,
`AgentPatchCommand` and `validate_patches` in
`crates/librepaper-room/src/room/agent.rs`. Sync does not get a second
apply path. The agent path is generalized into one core that both call.

1. **Rename to what it is.** `PatchRequest` becomes `SourceChange`,
   `AgentPatchCommand` becomes `SourceChangeCommand`,
   `apply_agent_request` becomes `Room::apply_source_change`, and
   `validate_patches` becomes `validate_source_change`. The module moves
   from `room/agent.rs` to `room/source_change.rs`. Agent-only code,
   operation key formats and MCP receipts, stays in `room/agent.rs`. No
   aliases for the old names.
2. **File operations.** `SourceChange` gains three lists next to
   `patches`: `create` with a path and text; `delete` with a path and,
   for a binary, `expect` holding the asset hash it must still have;
   `put_binary` with a path, the hash of an asset already uploaded, and
   `expect` for a replacement or nothing for a new path, which then
   requires absence. Validation covers valid paths, collision keys
   against the existing tree, no creation over an existing path, no
   patch and deletion on the same file, `max_files`, and the storage
   quota. They apply in the same mutation as the patches, all or
   nothing. Agents get them too, so an agent can create a file.
3. **Origin.** A trusted `Origin` set by the route, never read from the
   body. `Agent` keeps today's patch limits and records `agent`. `Sync`
   has no limits of its own, carries the provider name, and records
   `github`, which drives the editor notice and the activity record.
4. **Checks against the projection.** For `Origin::Sync`, a dependency's
   `file_hash` is compared with `digest_of` of the projected text, patch
   offsets are offsets in the projection and are mapped onto the stored
   text, and a patch touching a pending suggestion refuses naming it.
   `Origin::Agent` keeps checking the stored text as it does today.
5. **Every failure, not the first.** Dependency validation collects
   every touched file that changed and refuses once with the whole list.
6. **A transaction hook.** The command accepts a closure the route uses
   to write the link row after the apply and before the commit, so the
   sync record and the mutation are one transaction. Agents pass none.
7. **Tracked changes take a `SourceChange`.** `AgentTrackedCommand`
   accepts the same type, so a later "push as suggestions" needs no new
   apply code. File operations cannot be tracked changes; a change that
   has them and asks for review is refused with that reason.

## Self-hosting

A self-hosted LibrePaper registers its own GitHub App, as it registers
an OAuth app for GitHub sign-in today, and puts the app id, client id
and client secret in `keys.yaml`. GitHub Enterprise works by setting the
API host in the configuration. The shell's Content Security Policy
allows `connect-src` to that host, and to `api.github.com` on the hosted
instance.

## Limits

- GitHub: blobs up to 100 MB, a recursive tree listing up to 100,000
  entries or 7 MB before it is truncated, 5,000 requests an hour per
  user. A thesis with two hundred files costs two hundred blob calls on
  its first push and a handful afterwards.
- LibrePaper: `max_files`, 200 by default, `max_path`, the per-file
  limit and the owner's storage quota apply to pulled files as to
  uploads. A file over the size limits is listed as unsynced; a
  repository over `max_files` refuses the pull and says so.
- Nothing runs on the server per sync beyond one source-change apply
  and one row, the same cost as an agent edit.

## Security

- The server holds no GitHub token. The relay sees one in flight and
  must not log request bodies or responses on that route; a test asserts
  the token appears in no log line.
- `postMessage` from the relay page checks the opener's origin, and the
  opener checks the message's. State in the authorize redirect prevents
  a code from being planted.
- The browser's token lives in the account's local database, cleared on
  sign-out, never in a URL, never sent to the LibrePaper server. The
  Content Security Policy that protects the editor protects it.
- Pulled content is untrusted. Text goes through the same validation as
  an agent patch; binaries go through the asset route and its content
  check. Paths go through `validate_path` and the collision check.
- The source-change route and the link routes require editor on the
  document, like the socket.
- A GitHub user can push to the linked branch whatever LibrePaper's
  roles say. That is the repository owner's access control, not ours,
  and the panel says who connected and to what.

## Build order

Each step can be merged and shipped on its own.

1. **Core and route.** The shared source-change core of "Server apply",
   with no behavior change for agents; the `forge_links` table with its
   revision; the source-change route with `Origin::Sync` and the `sync`
   block; the link routes.
2. **Connect and pull.** The App registration, the relay, the consent
   popup, the pickers, the first comparison, the pull with whole-pull
   refusal, the export and the compare link, the export rule. Already a
   workflow: write locally, push to GitHub, pull in LibrePaper, resolve
   on GitHub when needed. The open questions, the popup and
   `postMessage` flow across browsers, Safari's popup blocking, and the
   exact 422 on a moved ref, are the first day of this step.
3. **Push.** The push, the moved-branch loop, the record.
4. **Docs.** `docs/github.md`: the consent click, what is synced, the
   folder link, the ignore file with its two-line example, the conflict
   workflow, and the self-hosting registration.

## Tests

Unit, in `web/`:

- blob ids match `git hash-object` for text with CRLF, text with LF and
  binaries; the content hash of a CRLF file equals the LF file's;
- the pull planner, on fixtures: GitHub-only change, LibrePaper-only
  change, both changed identically, both changed differently, added,
  deleted on one side, deleted and edited, a CRLF file edited only on
  GitHub is not a conflict, LFS pointer, oversize file, path outside
  the folder, path matched by the ignore file, path re-included with
  `!`, synced file added inside an ignored folder, a collision pair,
  an overlong path, a truncated listing, an export consumed, an export
  invalidated by later typing;
- the push and export change sets from a synced map and a project.

Server, in the gated Postgres suite:

- the source-change route applies all or nothing for an editor, refuses
  for a commenter, refuses with every mismatched file, replays by
  operation id, and writes the `sync` block only when the apply
  succeeds and in the same transaction;
- a stale revision answers 409 and applies nothing; two interleaved
  writers cannot both succeed;
- the relay returns the token to the opener and writes nothing to the
  database or the log;
- the link routes enforce editor and clear the synced state on
  disconnect.

End to end, against a real `admin serve` and a fake GitHub, a small Node
server speaking the endpoints above, including `truncated` listings and
the 422 on a moved ref:

- connect, pull, and the browser converges while typing in another
  file;
- a browser edit in `paper.qmd` then a GitHub commit to the same file:
  the pull refuses and applies nothing, the export creates the branch
  from the synced commit with LibrePaper's changes, a merge on the fake
  forge, and the next pull applies the merged file and clears the
  export; typing in the file after the export makes the next pull
  refuse again;
- push, then a GitHub commit, then push again: the second push pulls
  first and lands;
- a `Makefile` and a `renv/` folder survive a push untouched and never
  appear in the project; a `data/` folder of CSV files is synced, and
  ignored once named in `.librepaperignore`;
- a comment anchored on untouched text keeps its anchor through a pull;
- two browsers pull at once: one applies, the other gets 409 and ends
  with nothing to pull;
- a tab killed between the ref update and the record: the next pull
  records the baseline and applies nothing;
- a pull against a repository with `A.qmd` and `a.qmd` refuses naming
  both.

## Rejected designs

- **Tokens on the server.** The Overleaf and Typst model, with an app
  key or a per-user token in the database, so the server can sync
  without a browser. It gives sync while nobody is present, at the
  price of custody of credentials that open users' repositories. The
  owner's constraint was no such custody.
- **Server-side pull with a token passed per request.** The browser
  sends its token along with each pull and the server does the GitHub
  calls in memory. No storage, but the token crosses the server on every
  operation, and the browser can do the calls itself.
- **`isomorphic-git` in the browser.** Speaking the git protocol to
  GitHub from the browser. GitHub sends no CORS headers on its git
  endpoints, so it needs a proxy, and a proxy on our server sees the
  token.
- **Classic OAuth App.** Its `repo` scope is every repository the user
  can reach, read and write, which is why organizations block it. The
  GitHub App is limited to the repositories picked at install.
- **Partial pulls with per-file conflict state.** The first draft
  applied the clean files and kept conflicted ones for later, with three
  choices per file. Review found that the synced commit advanced past
  files that were never applied, so a reload lost the conflict and a
  push could overwrite GitHub. Durable per-file conflict rows would fix
  it, and whole-pull refusal makes them unnecessary.
- **The GitHub commit as the operation id.** Two different requests can
  arise from one commit, and the second would replay the first's
  receipt.
- **Recording the sync in a second request.** A closed tab or a second
  tab could leave the document and the record apart. The record now
  travels with the change.
- **The compare endpoint as the inventory.** It caps its file list at
  300 and a repository-wide change can hide the paper's files. The tree
  listing is complete or says it is not.
- **One hash per path in the map.** The GitHub blob id of a CRLF file
  never equals the hash of its LF text, so every such file looked
  locally edited forever.
- **`-librepaper` in `.gitattributes`** for exclusions, the helper
  design's mechanism. Nothing here runs git, so the browser would parse
  attribute syntax itself, and nobody knows that syntax by heart. The
  ignore file is gitignore syntax and a package that already parses it.
- **Merging both-sides changes in the browser.** A three-way merge in
  JavaScript applied as the user's typing. It would make Loro the
  arbiter of two people's text, which every git design here forbids.
- **Linking only an empty directory**, as Typst requires. The common
  layout is a code repository with the paper in a folder; the folder
  link serves it directly.
- **A git remote helper on every machine** (SPEC-git.md, rejected). Plain
  git against LibrePaper with a helper installed by the desktop app. It
  needed the app on every machine, wrote different commit ids on every
  machine, and spent most of its text recovering the clone's own
  bookkeeping.
- **A git remote hosted by LibrePaper** (drafted as SPEC-git-server.md,
  rejected, never merged). The server runs `git http-backend`, keeps
  packs on S3 and a cache under scratch, and merges pushes with
  `merge-tree`. It serves people without a forge and makes conflicts
  fail at push time, at the price of git on the VPS, packs in the
  bucket, a process cap, and about as much code again. The owner chose
  not to run a git server.

## Later

- **GitLab.** `gitlab.com/api/v4` and `gitlab.com/oauth/token` both
  answer browser requests, measured 2026-10-21, and GitLab supports
  PKCE, so the browser can exchange the code itself with a public
  application and the relay is not needed. Push is one
  `POST /projects/{id}/repository/commits` with `actions` and
  `last_commit_id` on every `update` and `delete`. That guard is per
  file, not per branch: a commit can land on a head that moved under
  it and carry another file's change, so after the commit the browser
  must read its parent and, when the parent is not the synced commit,
  run the pull planner from the synced commit to the new commit before
  recording. The tree listing is paged at a hundred and `compare` may
  be incomplete when `compare_timeout` is true, and it compares from
  the merge base unless `straight=true`.
- **Webhook, badge and automatic pull.** The App's webhook delivers
  pushes signed with `X-Hub-Signature-256`; the server verifies, sets a
  pending flag on the link and tells open rooms; browsers show a badge
  or pull on their own. Automatic pulls need the refusal to be quiet
  and the retry loop to back off. On GitLab a project hook needs the
  Maintainer role, so a Developer gets the buttons without the badge.
- **Push on a named version**, with the label as the message.
- **The ignore file, written for you.** An offer in the first
  comparison, checked by default when top-level folders hold nothing
  syncable, with the content shown and editable, and an "Ignore rules"
  editor in the panel that pushes a one-file commit.
- **Partial pulls** with durable per-file conflict rows, if whole-pull
  refusal proves too blunt.
- **Conflict resolution in LibrePaper.** `@codemirror/merge` already
  ships a side-by-side merge view; the conflict list could open it, and
  the result would go out as an ordinary export.
- **Sync while nobody is present**, if asked for: a per-project opt-in
  that stores an encrypted refresh token on the server for the webhook
  pull only, as a separate spec with its own custody rules.
- **Gitea and Forgejo** on self-hosted instances, once their APIs are
  checked for browser access.
- **A desktop path with the user's own `git`**, for forges behind a
  firewall and for people who prefer their SSH keys: the app would run
  `git fetch` and `git push` in a local clone and send the same source
  change. Not for now.
- **Rename detection**, when the core gains a rename operation that
  keeps comments attached.
