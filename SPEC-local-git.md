# SPEC: Desktop-local Git synchronization

Status: proposed 2026-10-22, revised the same day after review. It replaces
SPEC-git-forge-sync.md, whose browser GitHub flow was built on
`kata/vcfb-forge-sync` and never merged. What that branch keeps is listed
under "What the existing branch contributes".

## Summary

The desktop app connects a LibrePaper project to a branch of any Git remote
(HTTPS, SSH or a local repository), or to a folder inside that branch. Pull
and Push are buttons. Nothing synchronizes on its own.

System Git does the Git work in a private bare repository the app owns:
fetching, pushing, credentials, history and the three-way merge. LibrePaper
does two translations and nothing else. It turns the project into a Git
tree, and it turns the difference between two trees into one checked source
change, which the server applies to the live document as minimal character
patches. LibrePaper never writes a line-merge algorithm, never parses Git's
object format and never stores Git content on the server.

The server keeps a small shared connection record: the remote commit last
incorporated (the checkpoint), the set of paths synced at that checkpoint,
and a revision that every write must name. Everything else lives on the
desktop that did the work.

## Goals

1. Connect a project to any Git remote the user's own `git` can reach, and
   pull or push its supported files by hand.
2. Let Git own history, merging, transport and credentials. LibrePaper owns
   only which paths sync, how text is normalized, and how a tree difference
   becomes a checked change to the document.
3. Merge independent local and remote edits automatically. Overlapping edits
   stop for a person, without changing the project.
4. Keep comments and pending suggestions on untouched text attached, through
   minimal patches and the existing suggestion protections.
5. Leave credentials, local paths, Git objects and the operation journal on
   the desktop. The server stores no Git content and no secret.
6. Recover from a crash or a lost reply at any point without a duplicate
   edit, a lost commit or a guessed baseline.

## Non-goals for v1

- Sync from the browser. Browser editors keep collaborating and see what a
  desktop pulls; Git actions in the browser say they need the desktop app.
- Anything automatic: background pulls, webhooks, push on a named version.
- A general Git client: branch switching, history browsing, pull requests.
- Syncing or running the project from the user's own working checkout.
- LFS, submodules, symlinks, more than one branch per connection.
- An external merge tool. The built-in resolver is the only one in v1.
- Collecting credentials in LibrePaper. Git uses the machine's own.

## User experience

### Connecting

The Git panel of the desktop app opens from the Git tab in the side rail, or from "Git..." in the file menu.

1. The user enters an HTTPS or SSH URL, or picks a local repository with the
   system folder chooser.
2. They pick a branch and, optionally, a folder inside it. The folder is
   where the project's files live; everything outside it is never touched.
3. The panel shows the first comparison: files that will come in, files that
   will go out, files present on both sides with different content, files
   that will not sync and why, and the top-level folders with how many files
   each would sync.
4. When files differ on both sides, the user picks which side supplies them:
   "Keep LibrePaper's copies" or "Keep the repository's copies". The choice
   decides only those files. Files present on one side only always flow to
   the other side (see "First connection").
5. "Connect" records the connection and runs the first pull or push. Cancel
   changes nothing on either side.

A local repository is reached through a filesystem grant held by the desktop.
The server stores only a label (the folder's name) and an opaque connection
id, never the absolute path. Another computer picks its own local repository
for the same connection.

### Daily use

- **Pull** fetches the branch, merges it with what changed in LibrePaper
  since the checkpoint, and applies the result to the live document. The
  editor shows `<name> pulled 3 files from <remote label>`.
- **Push** pulls first, then shows the outgoing files and a commit message
  field (the default names the changed files), and pushes one commit on top
  of the remote head after the user confirms. If the remote moved before the
  push, it pulls again and asks for a new confirmation.
- **Conflicts** open the resolver. The project does not change until every
  conflict is resolved and the result applies.
- **Disconnect** forgets the shared record. It does not delete the private
  repository, unpublished commits, saved conflict sessions or the journal.

After connection, the panel shows the remote label, branch, folder, who
connected, the last pull and push, incoming and outgoing files, any saved
conflict or recovery state, and the actions that apply.

### What syncs

A **synced path** is a regular file under the linked folder that passes all
of these, judged against the commit being synced:

- its path is valid for a project: the deployment's text or asset extension
  lists, at most `max_path` bytes, at most eight segments, no dotfile segment;
- it is not inside a built-in environment folder (`renv/` at any depth);
- it is not matched by `.librepaperignore` in that commit, read at the
  repository root (paths relative to the root) and in the linked folder
  (paths relative to the folder), with gitignore syntax;
- it is not a symlink, a submodule, an LFS pointer, invalid UTF-8 for a text
  extension, or over the per-file size limit;
- no other synced path has the same collision key (NFC, case-folded).

A collision, an overlong path or more synced files than `max_files` refuses
the pull and names the paths. Everything else is unsynced: never fetched into
the project, never written, never deleted. The panel lists unsynced files.

## State

### Where things live

| Shared, on the server | Private, on each desktop |
|---|---|
| Connection id and revision | Bare repository and its objects |
| Remote label, branch, folder | Remote URL, local path grant |
| Checkpoint commit id | Credentials (the machine's Git and SSH) |
| Synced paths at the checkpoint | Operation journal and recovery refs |
| Who connected, last pull and push | Saved conflict sessions |

For a network remote, the remote label is the URL with any user info
removed. The desktop keeps the full URL; a URL carrying credentials is
refused at connect time.

### The connection record

One row per connected project, `sync_links`:

- `document_id`, primary key; `revision`, an integer the server increments on
  every write;
- `remote_kind` (`network` or `local`), `remote_label`, `connection_id`
  (opaque, used by desktops to match their local grant), `branch`, `folder`;
- `checkpoint`, the 40-hex id of the remote commit last incorporated, or null
  before the first sync;
- `synced_paths`, the project paths synced at the checkpoint, at most
  `max_files` of them;
- `connected_by`, `last_pull_at`, `last_pull_by`, `last_push_at`,
  `last_push_by`.

There are no per-file hashes. Git holds the content of every checkpoint, so a
desktop rebuilds the baseline from `checkpoint` and `synced_paths`. Per-file
hashes appear only inside one request, as dependencies that refuse the
request if somebody typed in a touched file meanwhile.

`synced_paths` is what makes a change of rules safe. Without it, a file that
became syncable because the deployment added its extension would be present
in the baseline and on GitHub but absent from the project, and the next push
would delete it. See worked example 5.

### The private repository

The desktop keeps one bare repository per connection under app data, keyed by
the server origin and the document id, so two servers never share state. Its
refs:

- `refs/librepaper/remote`: the last fetched head of the linked branch;
- `refs/librepaper/ops/<operation>/{base,local,remote,merged,published}`: the
  commits one operation built, kept reachable until the operation is
  reconciled;
- `refs/librepaper/conflict/<session>`: the inputs of a saved conflict.

No working tree is ever checked out. Every tree is built with a temporary
index (`git update-index --index-info`, `git write-tree`) and every commit
with `git commit-tree`. Merges use `git merge-tree --write-tree`, which needs
no index and no working tree.

## Snapshots and normalization

### The sync filter

`F(X)` is the set of synced paths at commit `X`, computed with the rules
above, the ignore files found in `X`, and the deployment's current rules.
All three snapshots of an operation are restricted with the filter of the
incoming commit, so the rules in force are the remote's, applied the same way
to every side.

### Normalized commits

`N(X, P)` is a commit built from commit `X` and a path set `P`. Its tree holds,
at project paths (the linked folder stripped), every path of `P` that exists
in `X`: text with every CRLF turned into LF, assets byte for byte. It has a
fixed author, committer and date, so the same inputs give the same commit id
on every machine. It is a private artifact and is never pushed.

### The three inputs

For an operation that fetched head `R`, with checkpoint `C` and synced paths
`S` on the server:

- **B′** = `N(C, S ∩ F(R))`: the baseline, restricted to paths that were synced
  and still are.
- **R′** = `N(R, F(R))` with parent B′: the incoming side.
- **L** = the project's files that pass `F(R)`, with parent B′: the local side.
  Text is the suggestion-free projection (suggested insertions left out,
  suggested deletions kept), LF by construction. Assets are their bytes.

Because L and R′ both have B′ as their parent, `git merge-tree --write-tree L
R′` finds B′ as the merge base by itself. Git 2.38 is the minimum version.

A path in `S` that no longer passes `F(R)` is in none of the three inputs, so
the operation leaves it untouched on both sides and it drops out of
`synced_paths`. A path that passes `F(R)` but is not in `S` is absent from B′,
so it reads as an addition on whichever side holds it.

### Building L in the desktop page

The desktop app runs the same web page as the browser, which already holds
the live document and computes the suggestion-free text
(`unsuggestedText` in `web/src/lib/track-changes.js`, with its Rust twin
`track::unsuggested`). The page hands the sidecar each synced file's text,
and each asset's sha256. The sidecar keeps a local table from asset sha256
to Git blob id; only an asset not in that table is fetched from the asset
route and hashed into the repository.

The server never builds L. It checks the result instead: every touched text
file carries the SHA-256 of the projected text it was planned against, and
the server compares it with its own projection.

### Line endings on push

A push starts from R's raw tree and writes only the synced paths whose
normalized content differs between the push result and R′. A CRLF file that
LibrePaper did not change keeps its CRLF on the remote. A file LibrePaper did
change is written with LF.

## Pull

1. Take the connection's local operation lock. Read the connection record
   (revision, checkpoint, synced paths).
2. Fetch the branch into `refs/librepaper/remote`. Fetch the checkpoint too
   if the repository lacks it. If it cannot be fetched, stop: see worked
   example 7.
3. Compute `F(R)`. A refusal (collision, overlong path, too many files) stops
   the pull with the paths named.
4. Build B′, R′ and L as above, and write a journal entry naming them.
5. Run `git merge-tree --write-tree L R′`.
   - Exit status 0: the output's first line is the merged tree M.
   - Exit status 1: conflicts. Save a conflict session and open the resolver.
     The project is unchanged.
6. Diff L against M over the paths of `F(R)`. Each changed text file becomes
   character patches computed in the page with the same diff the branch uses
   (`textPatches` on `@codemirror/merge`), plus one dependency carrying the
   SHA-256 of L's text. A new text file becomes `create`, a new or changed
   asset `put_binary` (uploaded first through the asset route), a removed
   file `delete`.
7. Send one source-change request with a fresh operation id and the `sync`
   block: the revision read in step 1, the new checkpoint R, and the new
   synced paths `F(R) ∩ R`. The server applies the change and writes the
   record in one transaction, or refuses whole.
8. On a dependency refusal, which means somebody typed in a touched file,
   rebuild L (step 4) and merge again, at most three times, then show the
   files.
   On a stale revision, read the record and start over from step 1.
9. Wait, with a bound, until the live document shows what the server applied
   (it returns over the socket like a coauthor's edit). Mark the journal
   entry done.

An empty diff still sends the request: the checkpoint moves to R with no
source change. That is how a pull adopts a push whose record was lost.

## Conflicts

`git merge-tree` lists each conflicted path with the blob ids of its base,
local and incoming versions (stages 1, 2 and 3), or which of them is absent.
The resolver reads those blobs from the repository. Nothing is checked out
and no conflict marker is ever written into the project.

Merges run with rename detection off, so a rename arrives as a deletion and an
addition.

For each conflicted path the resolver offers:

- text: a side-by-side view built on `@codemirror/merge`, filling the editor
  area: the repository's copy, read-only, beside an editable result that
  starts from LibrePaper's copy, with changed words highlighted and an arrow
  per difference that copies the repository's version into the result;
- deleted on one side and changed on the other: keep the changed file or
  delete it;
- binary: keep one side's bytes.

"Apply" builds M from the merged tree with the resolved paths written in, and
continues at Pull step 6 with the same L. If the dependency check refuses
because somebody typed in a touched file, the app rebuilds L and merges
again. A resolution carries over to the new merge for every path whose three
input blobs are unchanged; the others are shown again.

The session (its refs and the resolved texts) is saved as the user works, so
closing the app keeps it. Before applying a saved session, the app fetches
and checks the record: if the checkpoint or the remote head moved, it merges
again and carries over what still applies.

A resolved pull is an ordinary pull: the checkpoint moves to R and the
resolution lives in the project. Nothing is exported, so nothing has to be
recognized later on the remote.

## Push

1. Run a pull. If it refused or opened the resolver, stop. If it applied
   changes, its step 9 has already waited for them to reach the page.
2. Build L2 from the project, restricted to `F(R)`, with parent R′. If its
   tree equals R′'s, there is nothing to push.
3. Build the push tree: R's raw tree, with each synced path whose content
   differs between L2 and R′ written at `folder/path` (text as UTF-8 with
   LF), and each synced path absent from L2 removed. Every other path of R
   carries over unchanged.
4. Show the outgoing files and the message field. On confirm, write the
   commit with parent R (author: the LibrePaper editor as `Name
   <handle@users.noreply.<site>>`; committer: the machine's Git identity)
   and record it in the journal as prepared.
5. `git push <remote> <commit>:refs/heads/<branch>`, never with force. A
   non-fast-forward refusal fetches, goes back to step 1 and asks for a new
   confirmation.
6. Record the push: a write of the record naming the revision the pull in
   step 1 left, with checkpoint = the pushed commit and synced paths = the
   pushed commit's synced paths. Mark the journal entry done.

Typing that lands after step 2 is not in the commit and shows as outgoing at
the next push.

## The server side

The existing source-change route gains this `sync` block in place of the
GitHub one:

```json
{
  "revision": 7,
  "checkpoint": "<40 hex>",
  "synced_paths": ["paper.qmd", "refs.bib", "fig/a.png"]
}
```

It checks editor access, the change (dependencies against the suggestion-free
projection, patches mapped onto stored text, file operations), and the
revision, and writes source and record in one transaction under the
operation id. A retry with the same id returns the first receipt. A request
with a stale revision applies nothing.

`GET`, `PUT` and `DELETE /api/documents/{slug}/sync` read, write and clear
the record. `PUT` carries the revision it read and is how a connection is
created and how a push is recorded. Disconnect is `DELETE`.

The server never sees a URL with credentials, a local path, a Git object or
a Git command.

## Security and process boundaries

- The page talks to the sidecar through typed operations (connect, fetch,
  snapshot, merge, conflict blobs, build tree, commit, push, recover), never
  through Git arguments. Remote URLs are validated: `https://`, `ssh://`,
  scp-style SSH, or a granted local path. `protocol.allow` is limited to
  `https`, `ssh` and `file`, so `ext::` and `fd::` cannot run.
- Commands that only touch the private repository (`update-index`,
  `write-tree`, `commit-tree`, `merge-tree`, `cat-file`) run with
  `GIT_CONFIG_GLOBAL` pointing at an empty app file and
  `GIT_CONFIG_NOSYSTEM=1`, so no user or
  system configuration (attribute files, merge drivers, filters) applies.
  Nothing is checked out, so no smudge or clean filter can run.
- `fetch` and `push` keep the user's configuration, because credential
  helpers, `url.*.insteadOf` and proxy settings live there. They run with
  `core.hooksPath` pointing at an empty app directory, `GIT_TERMINAL_PROMPT=0`,
  and SSH in batch mode, so a missing credential is an error the panel shows,
  never a prompt that hangs. Error text has user info redacted.
- Pushing to a local folder runs the receiving side with
  `receive.denyCurrentBranch=updateInstead`, so a clean folder with that branch
  checked out gets the new files, and a folder with uncommitted changes refuses
  the push with a plain message. Remote (SSH/HTTPS) non-bare repositories still
  refuse a push to the checked-out branch; the panel suggests another branch.
- The bare repository and the journal have app-private permissions.

## Crash recovery

Each operation writes its journal entry before its first side effect and
keeps its commits reachable under `refs/librepaper/ops/<operation>/` until it
is reconciled. Phases: `planned`, `sent` (pull) or `prepared`, `pushed`
(push), `done`.

| Interrupted at | On restart |
|---|---|
| Fetch or merge | Start the operation over. Nothing changed anywhere. |
| Pull sent, reply lost | Resend the same operation id; the server returns the first receipt or applies it now. |
| Conflict session open | Restore it, fetch, check the record, merge again if anything moved. |
| Push prepared, outcome unknown | Fetch. If the commit is on the branch, continue as `pushed`; if not, it was never published: drop it or offer it as outgoing work. |
| Pushed, record not written | Read the record. If its checkpoint is the commit or a descendant, done. Otherwise write it with the record's current revision only if the checkpoint is still the commit's parent; if not, do nothing: the next pull adopts the push (worked example 8). |

Nothing in recovery forces a push, guesses a baseline or deletes a commit.
Anything that cannot be reconciled stays in the panel with its refs intact.

## Worked examples

Each example has a project folder `paper`, the deployment's default rules,
and a checkpoint `C` unless it says otherwise.

### 1. First connection, both directions

The repository has `paper/intro.qmd` and `paper/refs.bib`. The project has
`intro.qmd` (different text) and `notes.md`. There is no checkpoint.

The comparison lists `refs.bib` incoming, `notes.md` outgoing and `intro.qmd`
differing. The app builds a synthetic base for this one operation: for each
path present on both sides with different content, the base holds the copy
of the side the user did not pick; paths on one side only are absent.

- "Keep the repository's copies": the base holds LibrePaper's `intro.qmd`.
  The merge sees the repository change it, so the pull replaces the
  project's text with the repository's, creates `refs.bib`, and leaves
  `notes.md` as outgoing. The checkpoint becomes R; synced paths become
  `intro.qmd`, `refs.bib`.
- "Keep LibrePaper's copies": the base holds the repository's `intro.qmd`.
  The pull creates `refs.bib` and leaves `intro.qmd` alone; the push that
  follows writes `intro.qmd` and `notes.md`.

Both cases run the ordinary merge; only the base is special.

### 2. Independent edits merge

At `C`, `intro.qmd` has two paragraphs. A coauthor edits paragraph one in
LibrePaper. Someone pushes an edit to paragraph two. Pull: `merge-tree`
returns a clean M with both edits. The diff from L to M touches paragraph two
only, so one patch is sent, the coauthor's comment on paragraph one stays
attached, and the checkpoint moves to R.

### 3. Overlapping edits

Both sides changed the same sentence. `merge-tree` exits 1 and lists
`intro.qmd` with stages 1, 2 and 3. The resolver shows the three versions.
The user writes a result and applies. The pull sends patches from L to the
resolved text; the checkpoint moves to R. At the next push, the resolved
sentence is outgoing (it differs from R′) and is pushed on top of R.

If a coauthor types in `intro.qmd` while the resolver is open, the apply is
refused on its dependency, the app rebuilds L and merges again. If the typing
was in another paragraph, stages are unchanged for the resolved region, the
resolution carries over, and the second apply succeeds.

### 4. Typing during a pull

The pull plans patches for `intro.qmd` against L's text. Before the request
lands, a coauthor types a word in the same file. The server computes the
file's projected hash, finds it differs from the dependency, and refuses
naming `intro.qmd`. Nothing was applied. The app rebuilds L (now with the
word), merges again, and sends new patches. Their typing is never
overwritten.

### 5. Rules change between syncs

At `C`, synced paths are `intro.qmd`, `refs.bib`, `data/a.csv`. The repository
also holds `analysis.jl`, unsupported at `C`.

- The deployment adds `.jl` to its text list. At the next pull, `analysis.jl`
  passes `F(R)` but is not in `S`, so B′ lacks it, R′ has it and L lacks it:
  the merge reads it as a repository addition and creates it in the project.
  Without `synced_paths`, B′ would hold it, L would lack it, and the next
  push would delete it from the repository.
- Someone adds `data/` to `.librepaperignore`. At the next pull, `data/a.csv`
  is in `S` but fails `F(R)`, so it is in none of the inputs. It stays in the
  project and in the repository, untouched, and leaves `synced_paths`.

### 6. A CRLF file

`refs.bib` has CRLF endings in the repository. R′ holds it with LF, the
project holds it with LF, B′ holds it with LF. A pull that changed nothing in
it sees no difference. A push that did not change it writes nothing for it,
so the remote keeps CRLF. A push after editing it in LibrePaper writes it
with LF, which shows once on the remote as a whole-file change.

### 7. Rewritten history

Someone force-pushed the branch. A desktop that already has `C` merges as
usual: B′ comes from `C`, and the merge does not require `C` to be an
ancestor of R. A desktop that lacks `C` tries to fetch it; if the remote no
longer serves it, the panel stops and offers a rebaseline, which is worked
example 1 against the current head: the user sees the comparison and picks a
side for differing files. Nothing is guessed.

### 8. Two desktops and an interrupted push

Desktop A pushes P on top of `C`, then crashes before recording it. The
record still says `C`, revision 4.

- Desktop B pulls. It fetches head P. L (B's view of the project) already
  holds A's edits, because A pushed the live project's text. B′ is `C`, R′ is
  P: every change in P is also in L, so the merge equals L and the diff is
  empty. B sends an empty change; the checkpoint moves to P, revision 5.
- Desktop A restarts. Its journal says P was prepared. It fetches: P is on
  the branch. It reads the record: checkpoint P. Done.
- Had A restarted first, the record would still say `C`, revision 4, and `C`
  is P's parent, so A writes checkpoint P with revision 4.
- Had someone typed in a pushed file between A's push and B's pull, B's
  merge would carry that typing as an ordinary local change on top of P.

## What the existing branch contributes

Kept from `kata/vcfb-forge-sync`:

- the shared source-change core (`room/source_change.rs`): file operations,
  `Origin::Sync`, dependencies against the suggestion-free projection, every
  mismatched file in one refusal, the transaction hook, replay by operation
  id;
- `track::unsuggested` and `unsuggestedText`, with their shared fixture;
- the source-change route, with its `sync` block reworked as above;
- the revision-checked link table, reworked into `sync_links`;
- `textPatches` and the surrogate-safe patch merge;
- the sync rules module (`web/src/lib/forge/rules.js`) and its tests, moved
  to a transport-neutral name;
- the end-to-end harness pattern (`startDeployment`, a Node session on the
  room's socket).

Deleted: the GitHub REST client, the OAuth relay and token storage, the
`[forge.github]` configuration, the pull planner's map logic, the export
record and branch flow, the GitHub panel, the fake GitHub and
`docs/github.md`.

## Rejected designs

- **Browser GitHub sync** (SPEC-git-forge-sync.md). Every capability beyond a
  manual fast-forward (merging, another forge, SSH, local remotes) would be
  rebuilt per API in JavaScript.
- **Conflict branches with an export record.** Resolving outside LibrePaper
  needs the next pull to recognize the exported commit and use it as the
  merge base, which is shared state of its own. The in-app resolver makes the
  resolution an ordinary pull.
- **A per-file hash map on the server.** Git holds every checkpoint's content;
  hashes are needed only per request.
- **A document-wide revision as the concurrency check.** It would refuse a
  pull whenever anybody typed in any file. Per-file dependencies refuse only
  for the files a pull touches.
- **Temporary checkouts.** `merge-tree` and a temporary index need no working
  tree, so no filter runs and no checkout can escape its directory.
- **libgit2 or gitoxide.** Their SSH and credential handling differs from the
  user's own `git`, and that is where users' setups live.
- **The project execution folder as the checkout.** It would mix generated
  files, scripts and external edits into sync state.

## Build order

1. **Core.** Merge the source-change core, the suggestion-free projection and
   their tests from the existing branch on their own.
2. **State.** `sync_links`, its routes, and the reworked `sync` block.
3. **Sidecar.** The private repository, typed operations, URL validation,
   local grants, the configuration split, the journal and recovery refs.
4. **Merge.** Snapshots, normalized commits, `merge-tree`, the tree diff into
   a source change, the push tree.
5. **Panel and resolver.** Connect, first comparison, pull, push, the
   resolver, recovery state, disconnect.
6. **Docs.** Connecting each kind of remote, what syncs, the ignore file,
   conflicts, the minimum Git version and credential setup.

## Tests

- Unit: `N(X, P)` is deterministic and normalizes CRLF; `F(X)` for every rule
  in "What syncs"; the push tree preserves unsynced paths and untouched CRLF
  files; the tree diff becomes the expected source change.
- Sidecar, against temporary bare and ordinary repositories: fetch, non-force
  push, push into a local folder with the branch checked out (clean and with
  uncommitted changes), `protocol.allow`, no hook
  runs, no global attribute applies to a merge.
- Server, gated Postgres: the `sync` block is written only with a successful
  apply, a stale revision applies nothing, replay by operation id.
- End to end, against a real `admin serve` and a local bare remote: each
  worked example above as one test, plus comments on untouched text keeping
  their anchors through a pull.
