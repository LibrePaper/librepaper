# One Git panel

Status: proposed 2026-10-25 as Kata vwce, after the operator saw no git surface in the merged app. Builds on SPEC-sync.md (the two CLI commands and the GitHub Action) and takes the Git tab from SPEC-folder-git.md (branch kata/355w-folder-git). Replaces the Sync tab.

Revised 2026-10-26: the web project's panel connects a repository through the desktop app. The workflow and the token setup live in docs/git.md only, and the Action seeds its own state.

## Principle

Every project has one **Git** panel in the side panel, and it shows what git means for that project.

- A **web project** cannot run git itself; its repository is on GitHub and the Action does the work. The panel is where a repository is connected and seen: the link to the desktop app, and what the server knows about pushes.
- A **folder project** in the desktop app has a folder, so the panel runs git on it: status, commit, pull, push, branches and remotes, plus the LibrePaper link that today's Sync tab holds.

The rules from SPEC-sync.md stand: LibrePaper never merges text, never holds a forge credential, and never serves git. The panel runs the person's own git on the person's own repository.

## The panel in a web project

Shown to editors and owners (`!who.local && who.mayEdit`); readers and commenters do not see it. Sections, top to bottom:

1. **Repository.** "This project syncs with a git repository through its GitHub Action. Nothing runs on this computer." One line of status from the server: the newest version with reason `push` ("Last pushed by Alice, 2 hours ago: Reword the greeting") or "No push yet". This comes from `GET /history?named=1` as the History panel reads it; no new route.
2. **Connect a repository.** Shown in the desktop app only (`who.desktop`). Two buttons:
   - **Clone and connect…** takes an address and a place for the clone. It posts `projects/clone { url, project }`, which clones, links the clone to the project and pulls.
   - **Use a folder on this computer…** posts `projects/keep { url }`. The folder may be empty or a git repository. A folder already linked to another project is refused with 409 reason `linked`.

   In the browser, one sentence says to open the project in the desktop app.
3. **Docs.** A link to `docs/git.md`, which holds the workflow, the token setup and the GitHub Action.

## The panel in a folder project

Shown when `who.mayEdit && who.desktop && who.local`. The Sync tab's content moves here as the first section. Sections:

1. **LibrePaper.** Exactly today's Sync tab: sign in, Link or Publish, the linked project, "N changes here", "The web project changed", Pull, Push, Unlink; the git-folder notice is removed, because the same panel now offers git below.
2. **Repository.** When the folder has no `.git`: an **Initialize** button (`git init -b main`) and the sentence "Or clone a repository from the landing page." When it has one:
   - the branch with a menu to switch or create, and the remote with Set remote;
   - **Changes**: the files `git status` lists, each with its state and a Diff button that opens a two-pane diff of `HEAD` against the working tree in the editor area (the deleted `GitDiffWorkspace` and `git-diff.svelte.js` at 1e9e003b are the starting point, with a new data source);
   - a message field and **Commit** (stages everything by default; a file can be left out with a checkbox);
   - **Pull**, **Push**, and ahead and behind counts after a fetch;
   - **Sync**: one button that commits with the message "Edits from LibrePaper" when the tree is dirty, pulls, and pushes; it stops at the first step that fails and says which.
3. **Sign-in help.** When push or pull fails on authentication, the host-specific steps from the deleted `web/src/lib/gitsync/sign-in-help.js` at 1e9e003b, restored as it was.

### Rules for the repository section

- It is the person's repository: their identity, credential helpers, hooks, LFS and attributes apply. LibrePaper only makes git non-interactive (`GIT_TERMINAL_PROMPT=0`, `BatchMode=yes` for SSH), bounds run time, and refuses URLs that carry a password.
- When `user.name` or `user.email` is missing, Commit asks once and writes them into the repository's config.
- Pull needs a clean tree for tracked project files and no unsaved editor edits; the panel flushes the folder save first and refuses with the list otherwise. Push never pulls and never forces; "Pull first" when the remote moved.
- A conflicting pull is git's: the merge stays in progress with markers in the files, the panel lists the conflicted files and says "Resolve them in the editor, then Commit", and Commit completes the merge. This departs from SPEC-folder-git.md's resolver, which was deleted with the old Git sync; the editor is a text editor and git users know the markers. A later issue may bring a resolver back.
- Git 2.38 or later must be installed. Without it the section says so and names the docs. Bundled git is a follow-up.
- Nothing is written into the folder but what git writes.

## Clone from a Git URL

On the desktop landing page, beside **Open folder…**: **Clone a repository…** takes a URL (https, ssh or scp-like, no password in it), opens the native chooser for the parent folder, runs `git clone` into `<parent>/<repository name>`, creates the folder project and opens it. A clone of a repository that already holds LibrePaper state (`refs/librepaper/state` on the remote) is fetched too, so the LibrePaper section is linked at once.

## Desktop app

Routes under a folder project, origin-scoped like the others, each running in the project's root with the hardened runtime recovered from `crates/librepaper-companion/src/local/git/mod.rs` at 1e9e003b (finding git, the version gate, bounded execution, the non-interactive environment) and the URL validation and error classification from `validate.rs` and `remote.rs` there. Nothing from `objects.rs` or `storage.rs` comes back.

- `GET projects/<id>/git`: `{ repository: bool, git: { found: bool, version?: string, supported?: bool }, branch?, detached?, upstream?, ahead?, behind?, remote? (the one fetch, pull and push use), remotes: [{ name, url }], changes: [{ path, kind: "added" | "modified" | "deleted" | "renamed" | "conflicted", from? }], merging: bool, identity: bool }`.
- `POST projects/<id>/git/init`.
- `GET projects/<id>/git/diff?path=&from=`: `from` names the path `HEAD` is read at for a renamed file; `{ head: string | null, work: string | null, binary: bool }`.
- `POST projects/<id>/git/commit { message, paths? }`: answers the commit id, or 409 `{ reason: "identity" }` when the identity is missing; `POST projects/<id>/git/identity { name, email }` sets it in the repository config.
- `GET projects/<id>/git/branches`, `POST projects/<id>/git/switch { branch }`, `POST projects/<id>/git/branch { branch }`.
- `POST projects/<id>/git/remote { name, url }` (validated).
- `POST projects/<id>/git/fetch`: ahead and behind.
- `POST projects/<id>/git/pull`: refuses 409 `{ reason: "dirty", paths }` on tracked changes to project files; otherwise `git pull --no-rebase`; answers `{ status: "fast-forward" | "merged" | "up-to-date" | "conflicts", conflicts: [paths] }`.
- `POST projects/<id>/git/push`: sets the upstream on the first push; 409 `{ reason: "behind" }` when non-fast-forward; auth and network failures carry `{ reason: "auth" | "network", detail }` for the sign-in help.
- `POST projects/<id>/git/sync { message? }`: commit when dirty, pull, push; answers each step's outcome and stops at the first failure.
- `POST projects/clone { url, project }`: the native chooser, `git clone`, the link to `project`, the pull; answers like `projects/open`.
- `POST projects/keep { url }`: accepts an empty folder or a git repository; a folder already linked to another project is refused with 409 reason `linked`.

The folder's change poller treats what git writes as outside changes, as it does today, so a pull or a switch shows up in the editor. The render-output record and the opening-save rule are untouched.

## Web

- `web/src/lib/panels.js`: the `sync` tab becomes `git` ("Git", icon `git-branch`, restored to Icon.svelte), offered when `who.mayEdit && (who.local ? who.desktop : true)`.
- `web/src/components/reader/Git.svelte`: the panel with the two modes. The folder mode embeds today's Sync content (the controller `local-sync.svelte.js` stays) and adds the repository section driven by a new `lib/reader/local-git.svelte.js` controller over `lib/desktop/git.js` (one function per route). The web mode reads `/history` and `/share` through the existing API module.
- The File menu item "Sync…" becomes "Git…".
- Landing: **Clone a repository…**.
- Tests: unit tests for both controllers over fake routes; a browser test of the panel in both modes at 260 to 600 px; a companion route test for every git route against real git in a temporary repository (init, commit, identity, branches, remote, fetch and pull from a local bare repository, a pull with conflicts, push, push behind, auth error classification); a Node end-to-end test is not needed beyond the route tests, since the server is not involved.

## Docs

`docs/git.md` gains a short section "In the app" before "The two commands": what the Git panel shows in a web project and in a folder project, and Clone a repository. `docs/desktop-app.md`'s folder section names the Git panel instead of the Sync tab.

## Out of scope

Bundled git; an in-app three-way resolver; automatic background sync; the server knowing anything about GitHub.

## Done

`make test`, the browser suite, the companion route tests against real git, and a hands-on pass in the demo desktop: initialize, commit, set a remote to a local bare repository, push, pull a conflict and resolve it in the editor, Sync, Clone a repository.
