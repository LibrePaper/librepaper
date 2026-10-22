**Rejected 2026-10-21.** This design, a git remote helper on every machine,
was not built and will not be. The hosted-remote variant drafted the same
day, SPEC-git-server.md, was rejected with it and never merged. The git
work that goes ahead is SPEC-git-forge-sync.md. The text below is kept as
the record of what was considered.

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
`git pull --no-rebase --allow-unrelated-histories`. This revision reads
current committed `HEAD` policy during adoption, records server path
membership explicitly, handles local-only transitions as creates or
preserved files, and makes merge-oriented pulls opt out of rebasing.

The 2026-10-21 destructive-risk revision binds helper state to immutable
project identity, validates adopted fetch snapshots, adds a read-only push
preview, and documents conservative first-connection and recovery rules.

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
a first pull. Merge-oriented first pulls use `--no-rebase` explicitly, so
a user's `pull.rebase` setting cannot turn the required merge into a
rebase. What that pull needs depends on where the repository came from.

| Starting point | First pull |
|---|---|
| A clone of a repository that already syncs with this project, for example a coauthor's clone from GitHub | `git pull --no-rebase librepaper main` |
| A repository and a project that were never connected | `git pull --no-rebase --allow-unrelated-histories librepaper main` |
| A repository going into a new, empty project | `git pull --no-rebase --allow-unrelated-histories librepaper main` |

Each starts with
`git remote add librepaper "librepaper::https://<site>/docs/<slug>#k=<key>"`.
After the first pull, the user resolves any conflicts, commits, marks
local-only paths (see "Local-only paths") and pushes. Every later pull and
push is plain. Before the first real push, `git push --dry-run librepaper
main` previews the proposed path operations; then review warnings before
running the ordinary push.

Set the repository-local default with:

```sh
git config --local pull.rebase false
```

Do not change global Git configuration. A preexisting `pull.ff=only`
setting may still reject a merge commit; use
`git pull --no-rebase --no-ff librepaper main` in that case, retaining
`--allow-unrelated-histories` for an unrelated first pull. Users who
intentionally prefer rebase for daily work can continue using
`git pull --rebase`, with the same caveats as ordinary Git rebasing.

Before a first fetch into any existing repository, including one with
unborn `HEAD`, the helper requires a clean index and working tree.
`git status --short --untracked-files=all`
shows untracked files; ignored files are not covered and need a separate
backup. After dirty-state, target-identity, and candidate-snapshot
validation, and before the first import, a real fetch creates a uniquely named backup
branch at current `HEAD`. Rejected identity or snapshot checks leave all
refs, including backup refs, unchanged. Dry-run never creates this branch.
On collision, choose another name; never overwrite a ref. A clone or any
repository with unborn `HEAD` gets no empty backup branch, but still gets the
dirty-state check. The branch protects committed content only; it does not
save untracked, ignored, or dirty content, and a reflog is not a backup.

The helper never auto-stashes, resets, cleans, rebases, or rewrites the
user's branches. Its working-tree and index effects are limited to Git's
normal checkout/merge behavior for the requested command.

For never-connected histories, the ordinary merge is recommended. Advanced,
consequential alternatives are:

```sh
git pull --no-rebase --allow-unrelated-histories -X ours librepaper main
git pull --no-rebase --allow-unrelated-histories -X theirs librepaper main
```

These can discard conflicting content from one side. Review the merge and
backup before pushing; hash checks cannot detect a harmful resolution.

- **Already connected.** Fetch commits carry `LibrePaper-Project` and
  `LibrePaper-Project-Id`
  trailers. A first fetch adopts the newest such commit in `HEAD`'s history
  as base, so the pull is an ordinary three-way merge and needs no
  `--allow-unrelated-histories` flag.
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
- **No force.** `git push --force` is refused, see "Push". Advanced
  first-merge side choices are consequential, require review, and can discard
  conflicting content.

### Daily use

```sh
git pull librepaper main      # bring in everyone's LibrePaper edits
# edit locally, commit as usual
git push --dry-run librepaper main  # inspect the proposed operations
git push librepaper main      # review warnings, then send your commits
```

- A push that applies prints `LibrePaper: applied 3 files (paper.qmd,
  refs.bib, fig/plot.png)`.
- Coauthors with the project open see the text change live and a notice
  that reads `<name> pushed 3 files from git`.
- `git pull` makes a merge commit when the user also has local commits.
  Merge-oriented recovery commands use `--no-rebase` explicitly. For
  plain daily pulls, set repo-local `pull.rebase=false`; users who prefer
  rebase can explicitly run `git pull --rebase`.

### Refusals

Every refusal is a git push error on the `main` ref, with one line per
problem and a final line saying what to do:

```text
! [remote rejected] main -> main (LibrePaper changed since your last pull)
  paper.qmd was edited in LibrePaper since your last pull.
  Run: git pull --no-rebase librepaper main, then push again.
```

Identity mismatches name the expected and resolved project ids; an invalid
snapshot names the commit and validation failure; dirty first fetch names
the dirty paths and says to commit or move the files before retrying. The
full list is in "Push: refusal rules".

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
- `refs/librepaper/<remote>/state` points at a small metadata blob kept
  from garbage collection. It records the base commit id, `project_digest`,
  `synced_paths` (paths with known server baseline values in base, after
  LF normalization for text),
  `pending_create` (local candidate paths that must be created remotely),
  canonical origin and immutable project UUID, `policy_digest` (effective
  attributes plus `/api/config` text and asset extension lists), and hashes
  of preserved local-only and pending-candidate values in the base tree.
  The helper advances this ref and the base ref in one ref transaction.
- `.git/librepaper/<remote>/state.json` holds:
  - a cache of all fields in the durable metadata ref, including base OID,
    digest, `synced_paths`, `pending_create` and `policy_digest`;
  - `marks`, the path of the fast-import and fast-export marks file.

`project_digest` is the last full server projection digest associated with
an exchange. It does not assert that every path in the local base tree has
the same bytes as the server, because the tree can preserve local-only
files and pending create candidates.

Losing `state.json` loses no state that cannot be recovered. The helper
rebuilds it from the metadata ref. It never infers remote presence from a
blob in the commit tree. If both refs are absent, the helper must fetch
before push.

Before credential lookup or network use, the helper compares the parsed
canonical remote origin with durable metadata. A mismatch refuses locally.
The helper serializes operations for the same repository and remote from
reading these refs through recording the outcome. A cache whose base id
differs from the metadata ref is discarded. Missing or inconsistent durable
metadata refuses a push rather than guessing which paths were synchronized.

### When it moves

Base moves in exactly two cases:

1. **Fetch.** The new LibrePaper commit becomes base. Metadata records the
   fetched digest, the server paths whose exact values remain in the base
   tree as `synced_paths`, and preserved local candidates as
   `pending_create`. It also records current `policy_digest` and hashes
   for preserved local-only and pending-candidate values. The same
   `synced_paths` set is written to the fetch commit trailer for adoption
   by another clone.
2. **A push that succeeds.** The pushed commit becomes base, and the base
   ref and metadata ref move atomically. If the successful response has
   `base_matched: true`, record its new `project_digest`; otherwise record
   `null` so fetch reconciles edits in untouched files. Derive
   `synced_paths` and `pending_create` as specified in Push step 8. The
   pushed commit itself need not carry LibrePaper trailers. An authenticated
   empty Git request also succeeds and advances base metadata without a
   document edit.

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

`GET /api/documents/{slug}/project` returns `schema`, immutable
`project_id` (the immutable catalogue `IndexEntry.storage_id`), `project_digest`,
`tree` and `texts`. It accepts the login token and link key, in the same
way `/snapshot` does for `librepaper export`, and supports `If-None-Match`.
Binary files are read from `GET /api/documents/{slug}/assets/{sha}`, which
`librepaper export` already reads with the same pair.
A Git-identified request (`LibrePaper-Git: 1`) on a bound remote requires
`LibrePaper-Project-Id`; missing or mismatched ids refuse before conditional
responses or effects, including `304`. This applies to project and asset
reads, asset uploads, and pushes. The server gates Git asset upload before
upload admission, quota reservation, or blob persistence, and rechecks the id
for mutations/replay. Existing browser, export, and agent authentication
contracts remain unchanged. The initial unbound bootstrap project GET may
omit the id and returns the resolved identity without conditional reuse.
The helper checks that response identity before caching a digest or adopting
history. Durable metadata binds canonical origin plus project UUID; slug
and link key are not identity. Canonical origin has normalized scheme,
lowercase host and effective port, with default port omitted and no path,
key, or userinfo. Compare it with durable state locally before credential
lookup or any network request. A different origin refuses locally. Key
rotation or slug rename remains valid for the same UUID after server
validation. A different project requires a new remote name and the documented
first-connection procedure; never reuse or delete old state to bypass the
check. Push sends the expected id and does not perform a full-project read.

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

1. Read the metadata ref and `state.json`. Resolve current policy and the
   preservation source and pending candidates defined in step 3. If the server's
   `project_digest` equals the recorded full server digest, `policy_digest`
   matches the effective current policy, the pending-candidate set is
   unchanged, and the complete map of preserved paths and values equals
   the recorded map, report the recorded commit as `main` and stop.
   Comparing complete maps also detects removed paths. Nothing changed.
2. Download the project. Download each binary whose content hash is not
   already a blob in the repository.
3. Define the preservation source as committed `HEAD` for a connected
   repository, falling back to the previous base tree when `HEAD` is
   unborn. For a first fetch into an unrelated history, use an empty
   preservation source. Before building the tree, identify pending
   candidates as current-policy included paths present in the preservation
   source that were already pending or exist in the old base but are absent
   from its `synced_paths`. With no prior base, this set is empty. This
   presence rule prunes candidates deleted from current `HEAD`; a candidate
   now marked local-only is handled as local-only instead. Candidate values
   come from the preservation source, retaining edits in current `HEAD`.
   Build the tree in this order:
   - start with server paths eligible under current policy, at their project
     paths with mode `100644`; omit server paths that current policy marks
     local-only;
   - overlay every current-policy local-only path found in the preservation
     source, unchanged, and remove it from `synced_paths`;
   - overlay every known pending-create candidate from the preservation
     source, unchanged, and keep it out of `synced_paths`, even if the
     server projection already contains that path.

   `synced_paths` contains eligible server paths other than pending
   candidates, with their exact fetched values in the resulting base tree.
   An unrelated/root first fetch does
   not overlay any `HEAD` paths or mark them pending. Existing repository
   files survive the unrelated-history merge and become ordinary additions
   for push. A local-only server path absent from the preservation source
   is omitted from the fetch tree.
4. Write one commit through fast-import:
   - parent: base, or as "First fetch" says when there is no base;
   - author and committer: `LibrePaper <git@librepaper.org>`, dated at the
     fetch time;
   - message: `LibrePaper edits` plus a blank line and trailers,
     `LibrePaper-Project: https://<site>/docs/<slug>`,
     `LibrePaper-Project-Id: <project UUID>`,
     `LibrePaper-Digest: <project_digest>`,
     `LibrePaper-Synced-Paths: <canonical JSON array>`, and
     `LibrePaper-Snapshot: v1:<sha256>`. The array contains the byte-sorted
     managed paths whose exact fetched values remain in this tree. It is a
     single-line JSON array with canonical JSON string
     escaping, for example `["paper.qmd","refs.bib"]`. This trailer transfers
     membership when another clone adopts the fetch commit. The metadata
     ref also records pending candidates.

   `LibrePaper-Snapshot` is SHA-256 over UTF-8 bytes of the compact
   [RFC 8785](https://www.rfc-editor.org/rfc/rfc8785.html) JSON
   Canonicalization Scheme (JCS) serialization of this exact ordered array:
   `["librepaper-snapshot-v1", project_id, canonical_origin, tree_oid,
   object_format, synced_paths, project_digest]`. Render the SHA-256 digest
   as 64 lowercase hexadecimal characters. Read `tree_oid` from the actual
   candidate Git tree and `object_format` from the repository, never from
   claimed trailer values. Recompute the checksum with those values and the
   validated metadata. Use the full tree oid and object format `sha1` or
   `sha256`. Paths are byte-sorted and unique. The commit id is excluded to
   avoid circularity. The checksum binds
   immutable identity, origin, tree, membership, and server digest. It catches
   accidental edits, but is not a signature: deliberately false metadata
   can be regenerated with a matching checksum.

5. Move base: update the base ref, metadata ref, and `state.json`.

### First fetch

With no base, first parse the URL and compare its canonical origin with
any durable binding before credentials or network use. Bootstrap with a
project GET, resolve the UUID, and only then inspect candidate commits. The
helper decides the new commit's parent from the repository:

1. When `HEAD` is unborn, as in `git clone` or a fresh `git init`, the
   commit has no parent.
2. Otherwise the helper looks for the newest commit in
   `HEAD`'s history whose canonical origin and `LibrePaper-Project-Id` match the resolved
   target. Require exactly one of every snapshot trailer and validate the
   checksum and all snapshot fields before adoption; a slug change alone is
   allowed.
   When it finds one, that historical commit becomes base, with
   `project_digest` set to `null`. Fetch writes a new commit parented by
   that historical base. It downloads the current server projection and
   uses attributes in the adopted commit to reconstruct the old policy.
   The adopted fetch commit's `LibrePaper-Synced-Paths` trailer initializes
   `synced_paths`; the helper does not infer membership from tree blobs.
   Then committed current `HEAD` plus `.git/info/attributes` supplies
   current policy. A path in the adopted tree but absent from the trailer
   that is present in `HEAD` and included by current policy becomes
   `pending_create`; preserve its `HEAD` value until a create succeeds.
   Current local-only paths are also
   overlaid from `HEAD`.
   `HEAD` is never used as a synthetic parent. This keeps the genuine
   shared ancestor while applying later marks and preserving local work.
3. Otherwise the commit has no parent, and the helper writes to standard
   error: "This repository has its own history. To merge it with the
   project, run: git pull --no-rebase --allow-unrelated-histories librepaper main. See
   https://<site>/docs/git." Git then refuses a plain merge with its own
   message. Do not import synchronized `HEAD` files into this root commit
   or classify them as pending creates. The unrelated-history merge keeps
   those files as ordinary Git-side additions.

Before adoption, validate all required trailers exactly once, UUID and
origin, canonical unique synced-path membership, supported object format,
tree membership and modes, and checksum. Do not assign base or move any ref
until this validation succeeds. Every claimed member must exist with
regular-file mode `100644`. Malformed or duplicate metadata, missing members, wrong
modes, or a tree changed by amend, rebase, or cherry-pick while trailers
remain causes refusal. Message-only edits that leave snapshot tree and metadata
unchanged do not invalidate it. A matching-project invalid snapshot is a
hard error: preserve refs and working tree; do not fall through to an older baseline.
A different project or origin is never adopted. All produced snapshots
require this format; there is no deployed legacy format. Recovery preserves
the repository and restores original snapshot history from a trusted backup
or fresh LibrePaper clone, then explicitly merges the saved local branch.
Do not perform metadata surgery or hard-reset to hide the issue.

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
- On fetch and first-fetch adoption, the helper reads attributes from
  committed `HEAD`, falling back to base when `HEAD` is unborn, or an empty
  tree when neither exists. It adds `.git/info/attributes` and never reads
  working-tree attribute files. On push it reads the pushed commit's tree plus
  `.git/info/attributes`. `git check-attr --source` does this from git
  2.40. With an older git, the helper reads the `.gitattributes` blobs
  itself. `policy_digest` covers the effective result, including
  `.git/info/attributes` and the `/api/config` extension lists.
- These paths are always local-only and need no mark: `.git/`,
  `.gitignore`, `.gitattributes`, `.github/`, and `*.local`.
- A path whose extension is in neither `text_extensions` nor
  `asset_extensions` is local-only too, because LibrePaper cannot hold it.
  A `Makefile` or an `.Rproj` file needs no mark.
- Fetch carries local-only paths over from the preservation source defined
  in "Fetch: Steps", unchanged. Paths transitioning from local-only to
  synchronized are preserved as pending creates until confirmed by the
  server. A pending create is removed if the candidate is deleted from
  current committed `HEAD` or becomes local-only again. A preserved blob
  alone never establishes remote presence.
- Push skips them.

Marks cannot be inferred from the trees. A path missing from the project
could be local-only or could have been deleted in LibrePaper. The metadata
ref records which base paths were known server values and which local
candidates are pending creation. On first adoption without that metadata,
the helper reads the adopted commit's membership trailer, downloads the
project, and applies current committed `HEAD` policy plus
`.git/info/attributes`.

A file added locally is pushed like any other change, with no extra step,
unless it is local-only. A file a coauthor added to the shared repository
reaches LibrePaper on the next push from any clone, unless it is marked.
That is the same as the coauthor pushing it, and marking it and deleting it
in LibrePaper undoes it.

Changing a path from local-only to synchronized is an explicit transition.
The user commits the attribute change. If the path is absent from
`synced_paths`, the helper records it in `pending_create` and preserves the
local candidate across fetches. The next push submits it as text `create`
or binary `put_binary` without `expect`, even if the blob is already in the
local base tree. The server creates it only if that path is still absent.
If the path already exists remotely, the
server refuses the whole push and names the occupied path. It does not
adopt server bytes as the expected value and does not overwrite unseen
remote work. To recover, commit a copy of the local candidate under an
explicitly local-only path, remove or rename the candidate at its original
path, then run `git pull --no-rebase -X no-renames <remote> main`. Disabling
rename detection prevents Git from treating the backup as a renamed file
and applying the remote edits to it. The fetch drops the pending overlay
and lets the remote version appear. Resolve any
modify/delete conflict by keeping or merging the remote file with the
saved candidate, commit the resolution, then push. A successful create
moves the path from `pending_create` to `synced_paths`. Fetching before the create preserves
the candidate and the absent membership, so the next push still creates
it. Adding a `-librepaper` mark to an already synchronized path does not
delete the remote copy; the path becomes local-only for subsequent
operations.

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

`git push --force` and any other force update are refused; there is no
force-push bypass.

### Preview

`git push --dry-run <remote> main` uses remote-helper `option dry-run true`
and prints the bound expected identity from local state plus a per-path
plan of create, edit, delete, binary replacement, and skipped local-only
paths. It does not freshly resolve or authenticate the target, download the
project, or claim permission/dependency preflight. A rename appears
as delete plus create and warns that comments anchored in the old file are
lost. The preview never uploads assets, calls the mutating push route,
advances refs, marks or metadata, or mints/persists operation receipts.
It does not claim server dependency
checks passed. Use temporary marks and suppress private ref updates. Any
transport dry-run option is honored. Actual push recomputes the plan and the
server rechecks identity and dependencies. Before applying an actual push,
print warnings for new uploads, deletions/renames, and local-only to
synchronized transitions. There is no mandatory interactive confirmation.

### Inputs

Git sends a fast-export stream for `refs/heads/main`. The helper reads two
versions of every file:

- **base**: the file in base's tree, whose normalized content is a server
  baseline only for paths in `synced_paths`;
- **ours**: the file in the pushed commit's tree.

The helper does not read the server's current project. The server checks
it.

### Steps

Dry-run performs only the local planning and validation in steps 1 through
6, with policy changes held in memory. It prints the plan and exits before
asset uploads, Submit, or Record, including for an empty change set.

1. **Branch check.** Refuse any ref other than `refs/heads/main`. Refuse
   ref deletion and any force option.
2. **Ancestry check.** With no base, refuse and say to pull first. The
   pushed commit must descend from base. Otherwise refuse, even for
   `--force`, which git passes to the helper as `option force true`.
   LibrePaper never rewinds. A plain `git fetch` moves base without
   merging it, so the message says to merge rather than to fetch:
   "LibrePaper has edits you have not merged. Run: git pull --no-rebase librepaper
   main, then push again." When base is a root commit, the message names
   `git pull --no-rebase --allow-unrelated-histories librepaper main`
   instead.
3. **Reconcile policy and membership, then collect changes.** Evaluate the
   pushed commit's effective policy. Drop a `pending_create` entry if its
   candidate is deleted from the pushed tree or now marked local-only. For
   each included path present in ours but absent from `synced_paths`, emit
   a remote create even when its blob is unchanged from base: text uses
   `create`, and binary uses `put_binary` with `expect` omitted to require
   remote absence. For known synchronized paths, diff base against ours
   and emit patches, replacements or deletions. If current policy marks a
   previously synchronized path local-only, remove it from metadata and
   skip its content change without deleting the remote copy. Intermediate
   commits are collapsed. Only the net change is applied.
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
   hold in `expect`, on a `put_binary` or a binary `delete`. Text `create`
   and absent-path binary `put_binary` have no dependency. A path in
   `pending_create` is sent as a create even if its blob exists in base;
   absent membership requires a creation guarded by remote absence,
   rather than a patch inferred from the local blob's presence.
   If the server already has it, creation refuses and does not use current
   server bytes as an expected value. A known synchronized path is patched
   or deleted only with its base dependency.
   If that path's blob is missing from base, fetch and explicit recovery
   are required.
7. **Submit.** Send one change set (see "Server apply") with `base_tree`
   set to base's `project_digest`.
   If all changes were local-only and the change set is empty, still submit
   it to authenticate the link and confirm the exchange before moving base.
8. **Record.** Only after a successful server response, move base to the
   pushed commit and set
   `project_digest` as described in "When it moves", using the server's
   `base_matched` answer. On success, derive `synced_paths` from the old
   set filtered by pushed policy, minus successful deletions in the
   applied-path list, plus confirmed creates in that list. Prune resolved
   candidates from `pending_create`; retain candidates that were not part of the successful
   operation. If `base_matched` is false, leave `project_digest` null and
   retain untouched stale baseline membership. Update `policy_digest` and
   the hashes for preserved local-only and pending-candidate values from
   the pushed tree. Atomically update the metadata ref with the base ref.
   Report `ok refs/heads/main`. On a refusal, leave
   base alone and report `error refs/heads/main <reason>`.

There is no retry loop for conflicts. When a touched file changed, run
`git pull --no-rebase` and resolve the merge before pushing again. Other
refusals give the specific recovery described below, including preserving
the local candidate before resolving an occupied pending-create path.

### Push: refusal rules

| Rule | Checked by | Message names |
|---|---|---|
| Ref is not `main`, or is a deletion | helper | the ref |
| Pushed commit does not descend from base | helper | `git pull --no-rebase`, then push |
| A text file contains a line starting with `<<<<<<< `, `=======` or `>>>>>>> ` | helper | file and line |
| A text file is not valid UTF-8 | helper | file |
| A path is a symlink or a submodule | helper | path |
| A touched file changed in LibrePaper since base | server | every such file |
| A created path already exists in LibrePaper | server | path and `git pull --no-rebase` |
| The occupied path is a pending-create candidate | helper maps the server refusal | path and backup, remove or rename, pull, resolve, and retry instructions |
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
| Project identity is missing or mismatched | helper/server | expected and resolved project id; correct remote URL or use a new remote name |
| First fetch has dirty staged, tracked or untracked files | helper | paths; commit or move them and retry |
| Matching-project snapshot metadata is invalid | helper | commit and failed validation; restore trusted snapshot history and merge saved branch |

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
   - `delete`: an object with `path` and optional `expect`. Text deletion
     uses the ordinary whole-file dependency entry for that path and must
     omit `expect`. Binary deletion requires `expect` to contain the expected
     asset SHA-256.
   - `put_binary`: a path, the asset hash already uploaded through the
     existing asset route, and optional `expect` with the asset hash the
     path must hold now. Omit `expect` for a new path, which requires remote
     absence.

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
4. **Whether base matched.** On a successful apply, the outcome reports
   `base_matched: true` exactly when `base_tree` equals the full projection's
   `project_digest` just before apply. It does not assert that a preserved
   local-only path equals any server path. A refused occupied create has no
   `base_matched` result.
5. **Empty Git exchange.** `Origin::Git` accepts an authenticated empty
   change set. It makes no document edit and emits no editor notice, but
   returns the current `project_digest`, `base_matched` and operation
   receipt so a no-op push can confirm access before advancing local base
   metadata. Empty `Origin::Agent` requests keep their existing behavior.
6. **Checks against the projection.** This is the largest new piece of
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
7. **Every failure, not the first.** Dependency validation collects every
   touched file that changed and refuses once with the whole list, instead
   of returning at the first mismatch.
8. **Tracked changes take a `SourceChange`.** `AgentTrackedCommand` accepts
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
    "dependencies": [
      {"path": "paper.qmd", "file_hash": "<sha256 of base text>", "start": 0, "end": 0, "exact": ""},
      {"path": "old.tex", "file_hash": "<sha256 of base text>", "start": 0, "end": 0, "exact": ""}
    ],
    "patches": [{"path": "paper.qmd", "start": 120, "end": 131,
                 "exact": "old words", "replacement": "new words"}],
    "create": [{"path": "notes.md", "text": "..."}],
    "delete": [{"path": "old.tex"}, {"path": "fig/old.png", "expect": "<base asset sha>"}],
    "put_binary": [{"path": "fig/plot.png", "sha": "<new asset sha>", "expect": "<base asset sha>"}],
    "message": "Chapter 2 draft",
    "publish": "apply"
  }
  ```

  The route requires `LibrePaper-Project-Id`, resolves the slug, validates
  the immutable id before conditional behavior or effects, and rechecks it
  during mutation. It parses the body, sets `Origin::Git`, and calls
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

## Destructive risks and recovery

Deleting a synchronized file in Git deletes it in LibrePaper after a
successful push. A rename is delete plus create and loses comment anchors.
Git carries file content and history, not annotations or pending suggestions,
so it is not a full project backup. Fetch writes mode `100644`; an
executable bit is lost. Supported files in the pushed tree are uploaded
unless marked local-only, even if their commits were previously kept local
or in private repositories. Local-only is not a confidentiality guarantee
when publishing to GitHub or another host. `.git/info/attributes` affects
only that clone; use committed `.gitattributes` for shared policy. Treat the link key
as a secret; rotate or revoke it after exposure and update remotes. The
metadata cache is reconstructible, but refs hold safety-critical state.
`reset --hard` and `clean` can destroy local work; reflogs expire and are
not backups. Keep the dedicated backup branch and independent backups
before risky merges. Occupied-path pending-create recovery and
`-X no-renames` remain explicit as specified under "Local-only paths".

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
   - `/api/documents/{slug}/project` returns the immutable project UUID and
     checks mandatory `LibrePaper-Project-Id` on bound reads; bootstrap
     returns identity without conditional reuse;
   - `/api/documents/{slug}/project` accepts the login token with a link
     key, as `/snapshot` does;
   - the git projection without pending suggestions, its digest and file
     hashes;
   - Git asset reads and uploads require the expected project id and reject
     mismatch before conditional response or upload admission; browser/export
     routes retain their existing authentication contract.
2. **Helper: fetch.**
   - `git-remote-librepaper` installed by every package that installs
     `librepaper`;
   - remote-helper protocol with `import`;
   - base state, the `librepaper` attribute, line endings, file modes;
   - the git URL and sign-in on first use, refusing without a terminal;
   - text or binary from `/api/config`;
   - bootstrap identity resolution, checksum validation before adoption,
     current-HEAD policy, pending-create preservation, and a non-overwriting
     backup branch after validation and before import;
   - the durable metadata ref for base membership, digest and pending
     creates.
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
   - `export` capability, expected project-id header, ancestry check, local
     refusal rules, patch generation, per-file dependencies, state update;
     `option dry-run true` with temporary marks and no private-ref updates.
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
- the push route applies all or nothing and rejects missing or mismatched
  `LibrePaper-Project-Id` before effects, including on replay;
- a touched file changed since base refuses and names the file, while an
  untouched file changed since base does not refuse;
- a refusal lists every changed touched file, not only the first;
- a file with a pending suggestion: its dependency hash is the projected
  text's, and an agent patch to the same file still checks the stored text;
- replacing a figure and renaming a file each change `project_digest`;
- a Git asset upload with a mismatched id (including slug reuse) refuses
  before admission, quota changes or blob persistence, and a mismatched-id
  conditional read cannot return `304`;
- deleting a binary refuses when its asset hash is not the expected one;
- a text delete object has `path`, omits `expect`, and uses its dependency;
  a text delete with `expect` refuses. A binary delete requires `path` and
  `expect`; missing or stale hashes refuse;
- on a successful request, `base_matched` is true exactly when the supplied
  full projection digest equals the current projection digest;
- an occupied pending create refuses and has no `base_matched` result;
- an empty Git change set authenticates and returns digest, match result
  and receipt without a document edit or notice; empty agent behavior is
  unchanged;
- a creation over an existing path refuses with recovery instructions,
  and a binary replacement refuses when the current asset is not expected;
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

- reusing bound state with a changed origin or project UUID refuses without
  moving refs, marks or state, and without file writes/uploads; a slug rename
  or key rotation for the same UUID succeeds after server validation;
- first bootstrap resolves project identity before adopting history;
- shared checksum fixtures for SHA-1 and SHA-256 repositories and a non-ASCII
  path produce the specified JCS/SHA-256 bytes;
- amended/rebased snapshot tree, checksum/member tampering, duplicate or
  malformed trailers, missing members and wrong file modes refuse in local
  clones and clones fetched from GitHub; message-only edits with identical
  snapshot tree/metadata remain valid; the checksum contains no secret;
- backup branch names never overwrite collisions, and dirty tracked,
  untracked or staged files, including in unborn repositories, refuse before
  import; identity/snapshot rejection also leaves backup refs untouched;
- `git push --dry-run` reports target, operations, binary replacements,
  skipped local-only paths and rename warning while causing zero mutations,
  asset uploads or receipt persistence; an actual push still refuses a
  stale file after preview;
- actual pushes warn before new uploads, deletion/rename and local-only
  transitions; rename recovery retains the old file as delete/add;

- clone, then pull with no change, makes no new commit;
- local-only paths survive fetch and are never pushed;
- a local-only to synchronized transition is persisted as pending create,
  remains preserved across fetch, creates when absent remotely, and refuses
  with recovery guidance when already occupied remotely;
- removing only an attribute mark creates unchanged text and binary files;
  repeated fetches with unchanged pending candidates create no new commit;
- committing a candidate's removal clears its pending state, so the
  documented occupied-path recovery exposes the remote version through
  the normal merge with `-X no-renames`, preserving the backup's original
  content even when Git would otherwise detect a rename;
- a fresh clone adopting a fetch commit with a pending candidate reads
  `LibrePaper-Synced-Paths` and does not mistake that candidate for a
  server-backed path;
- changing committed `.gitattributes` after the adopted trailer commit
  affects first-fetch policy while retaining the historical commit as base;
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
- a clean temporary Git configuration performs the first unrelated pull
  with `--no-rebase`; a configured `pull.rebase=true` is overridden by
  that command, and a configured `pull.ff=only` is overridden by the
  documented `--no-ff` recovery command;
- `git config --local pull.rebase false` affects only this repository and
  makes the documented plain daily pull use a merge;
- each helper-side refusal rule produces its message;
- base moves on fetch and on an applied push, and not on a refusal;
- a no-op push that only skips local-only paths advances base and metadata
  only after an authenticated successful response;
- losing `state.json` recovers base membership, digest and pending creates
  from the metadata ref, including after a successful push;
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
- a coauthor's clone that adopts another clone's commit uses current
  `HEAD` attributes, preserves newly local-only files, and does not use
  `HEAD` as a synthetic parent;
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
  `paper.qmd`, then `git pull --no-rebase` merges (with conflict markers when the
  lines overlap), and the next push applies;
- a comment anchored on untouched text keeps its anchor through a push;
- a binary figure push replaces the figure in the editor;
- with the default quota, `git pull --no-rebase --allow-unrelated-histories` from an
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
