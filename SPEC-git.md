# SPEC: git through the project folder

Status: proposed 2026-10-14, not started.

Users ask for git integration. LibrePaper does not talk to git remotes.
Instead, the companion keeps a granted project folder in sync with the
editor in both directions. The person uses any git they like on that folder.
The server never holds a git credential, and concurrent edits are still
merged by Loro.

## Why

- **Credentials.** Pushing to GitHub from the server needs either the OAuth
  `repo` scope, which grants write access to every private repository the
  user has, or a GitHub App. GitLab, Codeberg and self-hosted remotes need
  stored tokens or deploy keys. Each option puts long-lived third-party
  secrets on the VPS. The companion runs as the user, so their own SSH keys,
  credential helpers and signing setup work unchanged.
- **Conflicts.** Git merges snapshots line by line and can refuse. Loro merges
  character-level operations and never refuses. A server-side two-way sync
  with a remote has to reconcile two histories, and it has to survive force
  pushes, rebases and loops. Overleaf's GitHub sync is known to be fragile for
  these reasons. This design turns folder changes into Loro operations
  applied at the frontier they were based on. Concurrent edits merge exactly
  as they do between two browsers.
- **Most of it exists.** Folder grants (`local/bindings.rs`) and outbound
  writes with `.local` copies (`sync_granted_folder`,
  `local/quarto/mod.rs`) shipped on 2026-10-14. The browser already rebases
  drafts with `forkAt` (`web/src/lib/proposals.js`).

## The user's experience

```sh
# In LibrePaper: File > Project folder > choose ~/papers/thesis (native dialog)
cd ~/papers/thesis
git init && git remote add origin git@github.com:me/thesis.git
# Edits made in LibrePaper land in the folder. Commit and push as usual.
git commit -am "Chapter 2 draft" && git push
# A coauthor pushed to GitHub. Pull, then bring the changes in.
git pull
# In LibrePaper: File > Project folder > Bring in folder changes
#   shows a per-file diff, the user ticks files, confirms
```

- Folder sync is desktop only, and only works while the companion runs.
- Bringing changes in needs an open editor tab.
- Viewers and commenters can sync outbound only. Only editors can bring
  changes in, because the edit goes through their normal session.

## Design

### Outbound: editor to folder

This is today's behaviour, with three changes.

1. **Sync on change, not only on render.** In a "keep folder in sync" mode,
   the browser sends `PUT /workspace?binding=` on a debounce whenever the
   shared files change, not only on render and live-preview sync.
2. **Pause during git operations.** The companion skips a write while the
   folder's `.git/` holds `MERGE_HEAD`, `REBASE_HEAD`, `CHERRY_PICK_HEAD`,
   `rebase-merge/`, `rebase-apply/` or `index.lock`. The sync reply says why,
   and the editor shows it.
3. **Optional commit on named versions.** When the user names a version, the
   companion can run `git add` on the manifest paths, then `git commit` with
   the version label as the message, as the user's own git identity. This is
   off by default. It never pushes. The companion runs the `git` on `PATH`. It
   does not embed a git library.

### The written record carries a frontier

Today `written/<binding>.json` maps a path to the digest the companion last
wrote. It becomes a map from path to digest and frontier:

```json
{ "paper.qmd": { "digest": "sha256:...", "frontier": [["<peer>", 1234]] } }
```

The browser sends the frontier its upload was taken from with every
`PUT /workspace`. The companion stores it opaquely and never interprets it.
There is no migration. A record entry without a frontier cannot be imported.
It is treated as an outside edit, exactly as today.

### Inbound: folder to editor

1. **Detect.** A new route, `GET /workspace/changes?project=&binding=`, lists
   manifest paths whose bytes no longer match the recorded digest. Each entry
   carries the path, the recorded frontier and the current bytes. The route
   reports the same git-operation guard as outbound, and nothing is offered
   while the folder is mid-merge or mid-rebase. Files the editor does not
   share are never listed.
2. **Review.** "Bring in folder changes" shows a diff per file. For each file,
   the browser forks the document at the recorded frontier (`forkAt`). That
   fork holds the exact text the companion wrote, so no copy of the base is
   stored anywhere. The diff is base text against folder bytes. Files are
   refused, with a reason, when:
   - the bytes contain git conflict markers (`<<<<<<<`, `=======`,
     `>>>>>>>` at the start of a line);
   - the bytes are not valid UTF-8 in a text file;
   - the frontier is no longer reachable because history was trimmed.
     `forkAt` fails on a shallow document, so the user is told to keep the
     folder copy as `.local` and redo the change by hand.
   Changes that rewrite more than half of a file are flagged but not refused.
3. **Apply.** For each ticked file, the browser applies the diff to the fork
   as minimal text operations (the same character diff the proposals path
   uses). It then imports the fork's new operations into the live document.
   Loro merges them with anything coauthors typed since the frontier.
   Comment and suggestion anchors survive because the operations are
   minimal.
4. **Acknowledge.** The browser then sends a normal `PUT /workspace`. The
   folder receives the merged text, and the record gets the new digest and
   frontier.

Figures and other binary files have no base to merge from. A changed binary
file is offered as a whole-file replacement, and the folder copy wins if the
user ticks it. A file deleted in the folder is offered as a deletion. A file
added in the folder is offered as a new shared file. Renames show up as a
delete plus an add, and both must be ticked.

### Pending imports are not clobbered

Today an outbound write overwrites an outside edit and keeps the old bytes as
`<name>.local`. With inbound sync, a changed file is a pending import. While
the editor has a binding with inbound enabled, the companion skips outbound
writes for files with a pending import and reports them. `.local` copies are
still made when the user declines an import, and for folders that are
outbound only.

The race accepted in Roborev 943 matters more now. An outside save landing
between the read and the replace is overwritten without a `.local` copy. It
would also never be offered for import. The write should re-check the digest
right before the rename and treat a mismatch as a pending import.

### One inbound folder per project

Suppose two coauthors have git checkouts of the same repository, and both run
`git pull` on the same commit. Each companion offers that commit's diff,
based on its own stale frontier. Loro keeps two concurrent identical inserts
as two inserts, so the text would appear twice. Two rules prevent this:

- **Skip no-op imports.** Before diffing, the browser compares the folder
  bytes with the document's current text. When they are equal, nothing is
  applied, and the record is refreshed with the current frontier. This covers
  the common case, where the editor already has the change because another
  folder brought it in.
- **One inbound folder per project.** The project stores which binding may
  bring changes in. It is a per-project setting, owned by an editor, and is
  shown in the Project folder menu. Every other granted folder is outbound
  only. A second editor can take the role over, which turns the previous
  folder outbound only. This makes the inbound folder the single place where
  git is reconciled with LibrePaper.

The setting stores no path, only that the role belongs to a given user's
companion and binding id. A path never leaves the computer.

## Later: watch mode

Once explicit import works, an opt-in watch mode can call the same pipeline
when files change. It debounces for a few seconds after the last write and
applies only files that pass every refusal rule, are not flagged as large
rewrites, and are not mid-git-operation. A branch switch rewrites many files
at once, so watch mode pauses and falls back to the review dialog when more
than a handful of files change in one burst.

## Not in this spec

- **Headless inbound sync.** Calling `document_apply` from the companion's
  automation peer works without a tab, but it refuses ranges that changed and
  cannot merge from a base. Making the companion a full Loro peer would be
  correct, but it couples the companion's releases to the document format and
  the room protocol. Revisit only if users need sync with no tab open.
- **A hosted git endpoint** (`git clone https://librepaper.org/git/<id>`) and
  **a one-way mirror to GitHub** through a GitHub App. Both remain possible
  later. The frontier-in-the-record idea carries over: each exported commit
  would record its frontier, and an incoming commit would be applied by
  forking at its parent's frontier.
- **Server-side two-way sync with a remote.** Rejected for the credential and
  history-reconciliation reasons above.

## Work

1. Companion: frontier in the written record; the git-operation guard on
   outbound writes; `GET /workspace/changes`; skip writes for pending imports;
   re-check the digest before rename.
2. Browser: send the frontier with each `PUT /workspace`; the "Bring in folder
   changes" dialog with per-file diffs and the refusal rules; fork, apply and
   import; the no-op check.
3. Server: the per-project inbound binding setting, editable by editors only.
4. Companion: optional commit on named versions.
5. Tests: two browsers plus one folder, with concurrent typing during an
   import; conflict markers refused; a trimmed frontier refused; two folders
   pulling the same commit produce no duplicate text; a branch switch during
   watch mode falls back to review.
