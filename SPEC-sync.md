# Sync through git

Status: proposed 2026-10-25 as Kata 6gmd, for review before any build. Supersedes `SPEC-local-sync.md` (px27) and the LibrePaper tab of `SPEC-folder-git.md` (355w). Lands as one set with local renders (0n7d) and local projects (pphs).

## Principle

The web project is the truth. Git is how it meets files on disk, other git users and a forge. LibrePaper never merges text, never holds a credential for anyone's forge, and never serves the git protocol. Two commands in the `librepaper` CLI move files between a working directory and a web project. A GitHub Action runs those two commands so the bridge is always on without anyone's laptop. A local folder on a web project is a git clone.

```text
Alice (VS Code, git) ──git push/pull──► GitHub repository ◄──Action: librepaper push/pull──► web project ◄── Bob (browser)
                                             ▲
Dana (desktop app, folder) ──Pull/Push buttons, same two commands, no git──────────────────────────┘
```

Settled earlier and kept: pure git with no LibrePaper-specific git commands, no file naming LibrePaper in the project tree, `main` only, a push touching a pending suggestion refuses, pulls never include pending suggestions, text and asset extensions come from the server.

## The two commands

```sh
librepaper pull [URL] [--dir DIR] [--no-merge] [--theirs PATH...] [--mine PATH...]
librepaper push [URL] [--dir DIR] [--message TEXT] [--author NAME] [--title TEXT]
```

`URL` is the project page, `https://<site>/docs/<slug>`. It is needed once; the link is recorded in the state. Credentials are the CLI's existing ones: `--token`, `LIBREPAPER_TOKEN`, or the token `librepaper login` stored. An `lp_` login token or an `lpt_` project token both work.

### State

One record per working directory:

```json
{ "site": "https://app.librepaper.org", "slug": "…",
  "pulled": 418, "head": "<git commit or null>",
  "files": { "intro.tex": { "kind": "text", "sha": "<sha256>" }, "fig/a.png": { "kind": "asset", "sha": "<sha256>" } } }
```

- `pulled` is the history `sequence` of the newest version this directory has taken in.
- `files` is the hash of every synced path as last pulled or pushed: SHA-256 of the UTF-8 text with pending suggestions undone, the server's `digest_of`, or the asset digest.
- `head` is the commit that represents the last pulled or pushed version (git repositories only).

In a git repository the record is the blob at `refs/librepaper/state`, so it lives in git's own directory, never in the tree, and a runner can push and fetch it. Outside a repository it is a JSON file in the user's state home keyed by the directory's absolute path.

### What a working directory holds

The CLI walks the directory with gitignore semantics (the `ignore` crate: `.gitignore`, `.git/info/exclude`, global excludes, hidden entries skipped). Of what remains, a path is synced when the server's path rules accept it (`paths::check`: no dotfiles, 8 segments, `max_path`), it has no colon (a Windows disk reads `C:/x` as another drive) and its extension is in `text_extensions` or `asset_extensions` from `/api/config`. A tracked file with the git attribute `-librepaper` is left out when git is present (`git check-attr`). Everything else is never read, written or deleted. Text files must be UTF-8; a file that is not is reported and skipped. The CLI compares and sends text with LF endings; when it rewrites a file it keeps that file's existing ending, and writes LF for a new file.

### pull

1. `GET /history?named=1&after=…` back to `pulled`. The versions after it, oldest first, are what to take in. Named labels and push labels only; render, agent and other unnamed labels are skipped.
2. For each version, `GET /history/{sha}?unsuggested=1` gives `files` and `texts`; missing assets come from `GET /assets/{digest}`.
3. **In a git repository**, pull is a fetch plus a merge:
   - Each version becomes one commit on `refs/librepaper/main`, chained on the previous one: tree = the version's files, author = the version's `by` (`<handle>@<site>` as the address), date = `at`, message = the label, trailer `LibrePaper-Version: <sha>`. A version this directory pushed (see push) is represented by the commit that produced it, so the chain stays related to `main` and nothing is committed twice.
   - Then `git merge librepaper/main` (fast-forward when `main` did not move; a merge commit otherwise). A dirty tree refuses before anything is fetched: commit first. `--no-merge` stops after the fetch. A conflicting merge is left to git exactly as `git pull` leaves it: the person resolves it with their tools.
   - The working tree is otherwise never written by pull; git writes it.
   - A path the working tree keeps out of the sync (ignored, marked `-librepaper`, not UTF-8, unreadable) is neither written to nor removed from the generated commits. Each tree keeps the base tree's version of it, so the merge leaves the file as it is.
4. **Outside a repository** (a desktop folder), pull writes the newest version into the folder: files changed only on the web are written, files changed only here are kept, deleted-on-the-web files are moved to the trash. A file changed on both sides is not written; it is listed in the result, and a second call with `--mine` or `--theirs` per path settles it. A path the web holds that the folder also has but the walk leaves out (ignored, hidden, not UTF-8) is never written over or trashed; it is listed as skipped and `pulled` stays where it was. There is no three-way merge.
5. `pulled` moves to the newest version, `files` to its hashes, `head` to the newest commit.

If no listed version has a sequence at or below `pulled`, history was trimmed. Pull then says so, takes the current head as a fresh base, and in a repository makes one commit for it.

### push

1. Walk the directory. Against `files`: new paths, changed paths (hash differs), deleted paths (recorded, now absent). A recorded path the walk no longer lists but that is still in the directory (now ignored, marked `-librepaper`, not UTF-8, unreadable) has left the sync. It is not a deletion, and the web project keeps the file.
2. Build one `SourceChange` with `consistency: unchanged_dependencies`: minimal byte-range patches for changed text (a line diff refined to the changed spans, `exact` carrying the base slice), `create` for new text, `delete` for removed text, `put_binary` for new or changed assets after `HEAD` and `PUT /assets`, binary deletes with `expect`. One `Dependency` per patched or deleted text file carrying the recorded hash. The operation key is `v2.<unix ms>.<32 hex>` in epoch `sync`, so a retry after a dropped connection replays the first receipt.
3. `POST /source-change` with `sync: { provider: "cli", message, author }` (below). The server applies everything or nothing.
4. On `409 changed` the reply names the paths; the CLI prints them, says "pull first", exits 1. On `409 suggestion` it names the path and the suggestion to accept or reject on the web. Nothing else is partial.
5. On success the reply carries the new label; `files` takes the pushed hashes for touched paths, `head` the current `HEAD` commit when in a repository. `pulled` does not move: the versions between were not taken in.

`--message` defaults to the `HEAD` commit subject in a repository (with "and N more" when several commits were pushed since `head`), `--author` to that commit's author name. Outside a repository the message is required and the author is the account.

A directory with no link and `--title` publishes: `POST /api/documents` (multipart, one part per synced file) creates the project, the reply's slug is recorded, and in a repository `head` is set to `HEAD` so the first pull is related history. A clone of a web project that starts empty does `pull` into an initialized repository; `refs/librepaper/main` becomes `main`. Linking an existing repository to an existing web project is the one case with unrelated histories; the first pull merges them itself with `--allow-unrelated-histories`, and files that differ on both sides are left as conflicts.

Bounds: the body is capped by the log quota (32 MiB), `max_files` by the server, one upload admission per asset per hour as today. No other limit.

## Server

Reused unchanged: `GET /history` (cursor, `named=1`), `GET /history/{sha}` (`projection_at` through `historical_fork_at`), `GET /assets/{sha}`, `PUT /assets`, `POST /api/documents`, `/api/config`, the source-change validation core in `librepaper-room` (path rules, `require_kind`, `digest_of`, `to_stored`'s suggestion refusal, idempotent operation keys).

Changed:

- `POST /api/documents/{slug}/source-change` takes `sync: { provider: "cli", message: string, author?: string }` and no longer needs a sync-link row, a `revision`, `connection_id`, `checkpoint` or `synced_paths`. `Origin::Sync { provider: "cli" }`, `unchanged_dependencies` required, agent caps off as for any sync. The label it writes is named `message`, with reason `push`, `author_label` = `author` when given (the git author), otherwise the caller's attribution, and the account behind the token in `author_account_id`. History shows "Pushed by Alice". The reply includes `{ label: { sha, sequence } }`.
- `?unsuggested=1` on `GET /history/{sha}` as well as `GET /project` (px27 commit 024999e2 adds it to `/project`; the same `track::unsuggested` applies on the forked document). Pull reads versions with it, so recorded hashes equal what a push is checked against.
- `lpt_` token admission (`agent_token_path_allowed`) widens for the Edit role only: `GET /history`, `GET /history/{sha}`, `GET /project`, `GET`/`HEAD /assets/{sha}`, `PUT`/`POST /assets`, `POST /source-change`. Read and Comment tokens keep the tools routes alone. One rule: syncing needs Edit, in CI as on the web.
- Token expiry: `365d` is accepted and the Share dialog offers it as "1 year", for CI. No token lives without an expiry.

Deleted, hard cutover (no users of web-project Git sync): `GET/PUT/DELETE /api/documents/{slug}/sync`, `sync_link.rs`'s git form and link hook, `storage/postgres/sync_links.rs`, the `sync_links` table by a new migration `0015_drop_sync_links.sql`, `Origin::Sync { provider: "git" }`'s only constructor. `routes.rs:67-71`, `server/mod.rs:99`, `engine/storage/postgres/mod.rs:25,55`, `engine/testing.rs:32` edited accordingly. `History.svelte` says "Pushed" where it said "Pulled from Git".

## GitHub Action

A public repository `LibrePaper/sync-action` with a composite action, pinned by tag, that downloads the matching `librepaper` release. Inputs: `project` (the URL), `token` (a secret). The workflow the docs show:

```yaml
on:
  push: { branches: [main] }
  schedule: [{ cron: "*/15 * * * *" }]
concurrency: librepaper            # one run at a time per repository
jobs:
  sync:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: LibrePaper/sync-action@v1
        with:
          project: https://app.librepaper.org/docs/<slug>
          token: ${{ secrets.LIBREPAPER_TOKEN }}
```

What the action does:

- Fetches `refs/librepaper/*` from origin, so the state survives runners. When the state is missing, it seeds it on that run: a first pull, a push of the branch, then a push of the repository's files. It stops with an error when both sides hold different versions of a file.
- On `push`: `librepaper push` with the pushed commit's subject and author. On `409 changed` it runs `librepaper pull --no-merge`, commits `librepaper/main`'s tree to a branch `librepaper/sync`, pushes it, opens or updates one pull request "LibrePaper changed files you also changed", and exits 1 so the run is visibly red. A person merges that PR in git; the next push event pushes the merged result.
- On `schedule`: `librepaper pull`. A clean merge or fast-forward is pushed to `main`. A conflicting merge is aborted and handled as above. Nothing to pull exits 0 with no commit.
- Always: `git push origin refs/librepaper/*`.

The runner's own `GITHUB_TOKEN` pushes to the repository; the LibrePaper token reaches the project. The server sees an ordinary CLI client. Forgejo Actions and GitLab CI run the same two commands under their own YAML; that is documentation.

## Desktop app

The sidecar is the same binary, so the two commands run in-process behind routes on a folder project:

- `GET projects/<id>/sync`: `{ linked, site, slug, pulled, changes: { here: N, web: bool } }`. The `web` flag comes from `GET /history?after=pulled&limit=1`.
- `POST sync/link { url }`, `POST sync/publish { title }`, `POST sync/unlink`.
- `POST sync/pull { mine?: [path], theirs?: [path] }`: the no-git form. Answers `{ written, trashed, kept, conflicts: [path], skipped: [path] }`. The page shows conflicts with the existing Keep mine / Load from disk prompt and calls again with the answers. The folder's change poll then reloads the editor.
- `POST sync/push { message }`: the page flushes the folder sync first so disk equals the editor, then calls. `409 changed` and `409 suggestion` are shown as they are.

Every call into the sync crate passes the folder's recorded render outputs (the files a local render wrote there, such as `main.pdf` and the files under `main_files`) as paths to leave out. The editor does not hold them, so they are never pushed, published, counted in "N changes here", or written by a pull. The record is the change poller's and lives in memory.

Authentication: the sidecar holds a device token from the same device flow backups use (`tokens.json`). Kata vhqg's narrower credential applies here too when it lands.

Page: a **Sync** section in the local project's side panel with Link or Publish, the linked project, "N changes here", "The web project changed", Pull, Push and Unlink. On a web project opened in the desktop app, **Work in a folder…** in the File menu asks for a folder, links it, pulls, and opens it as a local project.

A folder that is also a git repository is Alice's case: the Sync section says so and points to the Git tab or the terminal; pull and push there are the git form. The Git tab itself (plain git on the folder, `SPEC-folder-git.md`) is a separate issue and not part of this set.

## Deleted from the web app

`web/src/lib/gitsync/` entire (`sync.js`, `auto.js`, `bridge.js`, `rules.js`, `sign-in-help.js`, `patches.js`, `server.js`, `hash.js`, `identity.js`), `components/reader/Git.svelte`, `GitMergeView.svelte`, `GitConflictWorkspace.svelte`, `GitDiffWorkspace.svelte`, `lib/reader/git-auto.svelte.js`, `git-resolver.svelte.js`, `git-diff.svelte.js`; the Git imports and uses in `Reader.svelte`, `loadGitMergeView` in `lib/merge-editor.js`, `gitRequest` and the 502 kind handling in `lib/desktop/client.js`, the `git-branch` icon; tests `gitsync-*.mjs`, `git-sync.mjs`, `git-sync-fuzz.mjs`, `git-sync-harness.mjs`, `git-panel`, `git-diff`, `git-resolver`, and the `tools/dev/db` line that runs them. px27's `local-sync.js`, `Sync.svelte`, `change.js` and the injected resolver are not merged.

Deleted from the desktop app: `crates/librepaper-companion/src/local/git/` entire and `local/service/git.rs`, with the edits in `local/mod.rs`, `service/mod.rs` and `local/folder.rs` (`choose_git_directory`). The `Makefile`'s `git-test` target goes with the fuzz test.

About 8,100 production lines and 7,700 test lines, measured on main on 2026-10-25.

## Docs

`docs/git.md` is rewritten around the two commands and the Action, with the case studies below as its workflow section. `docs/nav.js`, `docs/desktop-app.md` and the site's comparison row ("Git: two commands and a GitHub Action; nothing to install for web coauthors") follow.

## Security position

- The server stores no forge credential, speaks no git protocol, keeps no repository, receives no webhook. The only server change that touches admission is the `lpt_` route list above.
- A repository that uses the Action holds one LibrePaper token scoped to one project with the Edit role and an expiry. Revoking it in Share ends the bridge. The token dies with the owner's session generation as today.
- Anyone with push rights on the repository can change the project through the Action. The docs say so where the Action is introduced; every such change is a named version with the git author, visible and restorable in History.
- Content from git goes through the same validation as any source change. Nothing in a pushed tree runs anywhere.
- An `author` field on a push is a name supplied by the caller, as in git itself. The account behind the token is recorded beside it.

## Case studies (acceptance criteria)

1. **Bob and Carol on the web.** No change. Named versions are what the cases below see as commits.
2. **Alice backs up a web project.** Empty clone, `librepaper pull URL` with a token, commit, push to GitHub; the Action then commits each named version. Or the reverse: a repository becomes a web project with `librepaper push --title`.
3. **Alice in VS Code, Bob in the browser.** Alice adds the workflow and a token. Her pushes reach Bob within a minute as a version in his history with her name. Bob's named versions reach her on the next `git pull` as commits by Bob. A file both touched refuses the Action's push; a pull request from `librepaper/sync` appears; she merges it in git. Bob sees none of this.
4. **Charlie, GitHub only.** Same repository, same Action, nothing to learn.
5. **Dana, desktop folder, no git.** Work in a folder (the File menu item), edit offline with local TeX, Push; Pull with a per-file choice when both sides changed a file.

## Done

- `make test`, the browser suite and `tools/dev/db test` green.
- Rust unit tests: diff to patches (including CRLF files, a file with only line-ending changes, identical content), the directory walk against `.gitignore` and the extension rules, state round trips in a ref and in a file, trim detection, version-to-commit chaining with a pushed version adopted by its commit.
- One end-to-end test driving the real CLI against a real `admin serve` (the full-deployment harness): publish from a directory, pull into another directory, edit on the web through the socket, pull with a commit per version, push with a refusal on a touched file, a refusal over a pending suggestion, a push that succeeds beside an untouched web edit, trim then pull, figure upload and replacement, retry of a push after a cut connection replays the receipt.
- Desktop tests: the sync routes on a folder project, the no-git pull with conflicts answered per path.
- A browser test for the Sync section's states.
- The action tested with a shell harness that runs its steps against a local bare repository and the same real server.

## Size

| Change | Production lines, estimate |
|---|---|
| Delete desktop Git sync, server link table, px27 | about −8,100, plus −1,350 unmerged |
| CLI `pull` and `push` (walk, diff, change builder, versions to commits, state) | +900 to 1,300 |
| Server: body form, `unsuggested` on history reads, admission, label fields | +150 to 250 |
| Desktop routes and Sync section | +300 to 400 |
| Action repository | about +120, YAML and shell |

## Decisions taken at build time (2026-10-25)

1. Token expiry for CI: `365d` joins the choices. No token without an expiry.
2. The `author` field is a caller-supplied name, as in git. The account behind the token is recorded beside it in `author_account_id`.
3. `librepaper pull` in a repository merges by default, as `git pull` does. `--no-merge` fetches only.
4. The Git tab for folders (`SPEC-folder-git.md`) follows later. This set ships plain git plus the two commands.
5. The Action's files live in `tools/sync-action/` in this repository and are published as `LibrePaper/sync-action` by the maintainer; the README has the commands.

## Out of scope

Bundled git, the Git tab, automatic bridging on a laptop, a forge mirror from the server, Forgejo and GitLab CI templates beyond a docs page, LFS.
