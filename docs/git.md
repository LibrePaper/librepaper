---
title: "Git"
---

Two commands in the `librepaper` CLI move files between a folder and a web project. `librepaper pull` brings the project's current version into the folder. `librepaper push` sends the folder's changes back. Git is yours. LibrePaper never merges text, and it never stores a credential for GitHub. A GitHub Action runs the two commands, so a repository and a web project stay in step with nobody's laptop involved.

## In the app

The **Git** panel is the guided way to do what the rest of this page describes by hand. It appears in a web project for its editors and owners.

### In a web project

The panel shows the **last push**, the newest version pushed from Git, such as "Last pushed by Alice, 2 hours ago: Reword the greeting", or "No push yet".

In the desktop app, the panel also connects the project to a git repository. The panel shows the project, the repository and the branch.

- **Connect** takes the repository's address. The desktop app keeps a copy of the repository on your computer, out of sight.
- **Get repository changes** imports the repository's changes into the project and its History.
- **Send project changes** sends the project's files to the selected branch of the repository.
- When a file changed in both the project and the repository, the panel lists it. Choose **Keep project's** or **Take repository's** for each file. Then choose **Get repository changes** or **Send project changes** again.
- **Use a local repository** picks a git repository on this computer. The buttons then work in that folder directly, with no remote. Only committed files are sent, and uncommitted changes to tracked files stop the transfer. Conflicts are left in the repository for your own tools.
- **Disconnect** forgets the link. The copy on your computer, or the local folder, and the project stay as they are.

The connection belongs to this computer. A coauthor on another computer connects on their own.

In the browser, the panel says to open the project in the desktop app. Git signs in with your own credential helper or SSH key.

### Credentials

Git signs in to the remote with your own credential helper or SSH key. LibrePaper holds no credential for GitHub or any other host. When a pull or a push fails on sign-in, the panel shows the steps for your host and your system, and Try again repeats the command.

## Requirements

- The `librepaper` command. Install it with a desktop app package: Homebrew, Scoop, the Windows installer, the `.deb` or `.rpm` package, or Nix. See [Install the desktop app](desktop-app.html#install-the-desktop-app). The plain macOS `.dmg` does not put it on your PATH. On a server, [install the command alone](cli.html#install).
- Git, if the folder is a repository. A plain folder needs no Git.
- A token, or a login on this computer.
  - A project token: open the project, then Share, then the Agent tokens tab, then Create. Choose the Edit role and an expiry of up to 1 year. [Agent tokens](agents.html#agent-tokens) has the details. A project token works for a project that exists; publishing a new one needs `librepaper login`.
  - `librepaper login` signs you in on this computer. The login lasts 90 days.

Give the token to the CLI with `--token`, or with the `LIBREPAPER_TOKEN` variable. After `librepaper login`, you need neither.

```sh
# Paste the token once for this shell. Nothing is echoed or saved in shell history
read -rs LIBREPAPER_TOKEN && export LIBREPAPER_TOKEN
```

## The two commands

The first `pull` takes the project's address. The folder remembers it, so later runs need no address.

```sh
# First run in a repository: give the project's address and the branch to sync
librepaper pull https://app.librepaper.org/docs/<slug> --branch main

# Send this folder's changes to the project
librepaper push --message "Tighten the abstract"
```

### pull

```sh
# In a repository: fetch the named versions and the current version, then merge
librepaper pull

# In a repository: fetch only. Merge with git later
librepaper pull --no-merge

# Outside a repository: keep your copy of a file that changed on both sides
librepaper pull --mine intro.tex

# Before the first pull: list what it would change. Writes nothing
librepaper pull https://app.librepaper.org/docs/<slug> --preview
```

- In a repository, pull fetches each named version as a commit on `librepaper/main`. Then it adds the current project as one commit when its files differ. See [Versions and commits](#versions-and-commits).
- Then pull merges `librepaper/main` into your branch.
- If the merge conflicts, git leaves the conflict for you. Resolve it with your tools.
- The first pull into a repository that shares no history with the project merges the two histories itself. Files that differ on both sides are left as conflicts.
- In a repository, pull refuses to start with uncommitted changes. Commit first.
- Outside a repository, pull writes the current project into the folder. A file that changed only on the web is written. A file that changed only in the folder is kept. A file deleted on the web goes to the trash.
- A file that changed on both sides is not written. Pull lists it. Run pull again with `--mine PATH` to keep your copy, or `--theirs PATH` to take the web copy.
- A file the folder itself excludes (ignored by `.gitignore`, hidden, or not a file the project can hold) is left alone even when the web has it. Pull lists those too.
- A pull with nothing new, when the project has not changed since the last pull, reads nothing else from the project.
- `--preview` lists what a first pull would do. It shows these paths:
  - only in the project;
  - only in the folder or repository;
  - different on both sides;
  - the same on both sides;
  - excluded by the transfer rules;
  - blocked, with the reason.
- A preview writes no files, saves no link, and binds no branch. The real pull reads the project again, so its result can differ from the preview if the project changed.

### push

```sh
# Send changes. In a repository, the message defaults to the latest commit subject
librepaper push

# Publish a folder that has no project yet, as a new project
librepaper push --title "Thesis draft"
```

- Push sends new, changed and deleted files in one request. The server applies all of them or none.
- If a file you changed also changed on the web, push is refused. The CLI names the file and prints "Pull first." Pull, then push again.
- If a file you changed has a pending suggestion, push is refused. Accept or reject the suggestion on the web, then push again.

Push takes these options. Pull takes `--dir` and `--token` too.

- `--dir DIR` chooses the folder. It defaults to the current folder.
- `--message TEXT` names the version. In a repository it defaults to the latest commit subject. If several commits came since the last sync, it adds "and N more". Outside a repository it is required.
- `--author NAME` sets the author. In a repository it defaults to the author of the latest commit. Outside one, the author is your account.
- `--title TEXT` publishes a folder with no project as a new project. This needs `librepaper login`, since a project token belongs to one existing project.
- `--branch BRANCH` names the branch to send from. See [Branch binding](#branch-binding).
- `--token TOKEN` gives the token for one run. `LIBREPAPER_TOKEN` works too.

### Branch binding

A repository is bound to one branch and one project. The first real transfer needs `--branch`. After that, the CLI keeps the branch in the sync state.

```sh
# First real transfer: bind the repository to main
librepaper pull https://app.librepaper.org/docs/<slug> --branch main

# Later runs use the bound branch. Naming it again is allowed
librepaper push --branch main
```

- The branch must be the one checked out. If you are on another branch, the command stops and names the bound branch. LibrePaper never switches branches for you. Check out the bound branch with `git switch`, then run the command again.
- A detached HEAD is an error. Check out the branch first.
- Other branches stay available for experiments. They are not synced. Merge changes into the bound branch with git, then transfer.
- The sync state also records the project. A repository bound to one project is refused for another. Nothing is overwritten.
- A plain folder has no branch. `--branch` is refused there.

### Pending push

A push saves a small pending request in the sync state before it sends it. If the command stops before the web project confirms the send, run `librepaper push` again. It sends the pending request first, then the new changes.

```sh
# A push stopped before it finished. Run it again to send the pending request
librepaper push
```

- While a push is pending, `pull` refuses to start. Its error says to run `librepaper push` first. Pull, link and unlink stop in the same way.
- Recovery works only with the same account, on the bound branch.
- The request stays in the folder's state file, or in `refs/librepaper/state` for a repository. If that state is lost, so is the request.
- The web project can forget the request after history is trimmed. Then a retry cannot tell whether the first send was applied. If the same change is accepted again, it is recorded again. Recovery is not exactly once.
- A retry can still stop with "Pull first." if a file changed on the web. Pull, then push again.

## GitHub Action

The Action runs `pull` and `push` for a repository on GitHub. Nothing has to run on your laptop.

1. Add a project token as a repository secret named `LIBREPAPER_TOKEN`. Use Settings, then Secrets and variables, then Actions. A token for one project keeps the Action away from every other project.
2. Save this workflow as `.github/workflows/librepaper.yml`. Put your project's address in `project`.

```yaml
on:
  push:
    branches: [main]
  schedule:
    - cron: "0 * * * *"          # hourly
  workflow_dispatch:
concurrency: librepaper
jobs:
  sync:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
        with:
          ref: main
          fetch-depth: 0
      - uses: LibrePaper/sync-action@v1
        with:
          project: https://app.librepaper.org/docs/<slug>
          token: ${{ secrets.LIBREPAPER_TOKEN }}
          branch: main
```

The Action takes three inputs.

- `project` is the project's address, `https://<site>/docs/<slug>`.
- `token` is the secret. The workflow passes `LIBREPAPER_TOKEN` to it.
- `branch` is the branch to sync. Keep it equal to the branch that `actions/checkout` checks out with `ref`. The Action stops if the checkout is on another branch.

GitHub gives each run its own `GITHUB_TOKEN`. The `permissions` line lets it write to the repository and open pull requests. You do not add it.

Each scheduled run spends GitHub Actions minutes. The default is hourly. GitHub also delays frequent scheduled runs. Do not set an interval shorter than 15 minutes.

On its first run, the Action links the repository to the project itself. It pulls the project's versions into the branch, pushes the branch, and pushes the repository's files that the project lacks. If a file differs on both sides, the run stops with an error. A person then merges it once on a computer.

Each event moves files in one direction.

| Event | Direction | What it does |
| --- | --- | --- |
| Push to `main` | Repository to project | Sends the pushed commit as one named version. Its subject names the version, and its author becomes the version's author. |
| Schedule | Project to repository | Pulls the project and merges it into the branch. A clean merge is pushed to the branch. When nothing is new, the run ends with no commit. |
| Manual run | Project to repository | Does the same as the schedule. |

What else happens:

- **When a push is refused**, the Action commits the web's files to a branch named `librepaper/sync` and opens one pull request titled "LibrePaper changed files you also changed". The run then fails on purpose, so it shows red. Merge the pull request in git. The next push to `main` sends the merged files.
- **When a scheduled pull conflicts**, the Action aborts the merge and does the same.
- **When a send is pending from an earlier run**, a scheduled or manual run stops with an error. Re-run the push workflow, or run `librepaper push` in a clone.
- **When `main` moved during a scheduled run**, the push to `main` fails, the LibrePaper state is not published, and the next run repeats the pull from the same point.
- **When an older push run is started again**, the Action sees that its checkout is no longer the tip of the branch and pushes nothing, so an old commit never rolls the project back.
- **On every run that ends well**, the Action pushes the LibrePaper refs back to GitHub. The next run, on a new machine, starts from the same state.

Anyone with push rights on the repository can change the project this way. Each change is a named version under the git author. You can see it and restore it in History. To stop the Action, revoke its token in Sharing.

The Action reports the repository and the project separately. A repository push that a protected branch rejects does not undo a web change that already went through. The Action never force-pushes the branch or bypasses branch protection.

## Common workflows

1. **Bob and Carol write on the web.** They edit and comment in the browser, and nothing changes for them. Their named versions become commits in the repository on the next hourly run.
2. **Alice backs up a web project.** She pulls the project into a new repository with her token, pushes the repository to GitHub, and adds the token and the workflow. The first run does the rest. From then on, the Action adds each new named version as a commit. She can also go the other way. `librepaper push --title` in a repository that holds her paper creates the project.
3. **Alice works in VS Code, and Bob works in the browser.** Alice adds a token and the workflow, and the first run does the rest. Without the Action, she can connect the repository in the web project's Git panel in the desktop app, and press **Get repository changes** and **Send project changes** herself. Her pushes reach Bob at once, as versions in his history under her name. Bob's named versions reach the repository on the next hourly run, and her next `git pull` brings them in as commits by Bob. If they both changed a file, the Action's push is refused. A pull request from `librepaper/sync` opens, and Alice merges it in git. Bob sees none of this.
4. **Charlie works on GitHub only.** He works in the same repository, with the same Action. He has nothing new to learn.

## What syncs

These files sync.

- Text files with an extension the project accepts, such as `.tex`, `.typ`, `.qmd`, `.md`, `.bib` and `.csv`.
- Figures and fonts with an accepted extension, such as `.png`, `.jpg`, `.svg`, `.pdf` and `.woff2`.
- The server sets the list. The Files panel names the allowed extensions when it skips an upload.

These files never sync.

- Files that a `.gitignore` excludes. The CLI does not read them.
- A tracked file with the Git attribute `-librepaper`. Add a line like this to `.gitattributes`.

```text
# .gitattributes: keep this file out of the web project
notes.tex -librepaper
```

- Names that start with a dot, such as `.gitignore`.
- Paths with more than eight segments, and paths the server rejects.
- Files in formats the project does not accept.
- Anything outside the folder you chose.

These files and features stay where they are.

- Comments, suggestions and chat. Pull leaves pending suggestions out of the text it writes.
- Project history. Git receives the named versions and the current version as commits.
- Build outputs, such as `.aux` and `.log`. They stay on your computer. A file a local render wrote into the folder, such as the PDF, is neither imported into the project nor synced.

## Versions and commits

- In a repository, each named version becomes one commit when you pull. The commit's author is the person who saved the version, shown as `handle@site`. Its date is when the version was saved. Its message is the version's name.
- Each commit has a `LibrePaper-Version` trailer that names the version.
- After the named versions, pull adds the current project as one commit named "Current version" when its files differ from the last commit it imported. This holds edits that were never named. Pending suggestions are left out, as before.
- A quiet pull, where the project has not changed since the last pull, reads nothing else.
- Renders stay out of the project, so they never become commits.
- A push makes one named version in History. The name is the commit message. History shows it as pushed by the git author.
- A version that a push from this folder created is not committed a second time. The commit you pushed stands in for it.
- If someone trims the project's history past your last pull, pull says so. It takes the current files as a new starting point. In a repository, that is one commit. Commits you already have stay as they are.

## Limits and troubleshooting

Limits:

- The size of one push is capped by the project's log quota, 32 MiB by default.
- A project holds 200 files by default. A push over the limit is refused.
- Text files must be UTF-8. A file that is not is reported and skipped.
- The CLI keeps the line endings of each file it rewrites. New files get LF.

### Pull refuses to start

- The repository has uncommitted changes. Commit them, then pull again.
- A push is pending. Run `librepaper push` first, then pull again.
- The checkout is not the bound branch, or it is detached. Check out the bound branch with `git switch`.
- The repository has no bound branch. Run pull once with `--branch BRANCH`.

### Push says "Pull first."

Someone changed a file on the web. Pull, resolve any conflict, then push again.

### Push says a suggestion is pending

Accept or reject the suggestion on the web. Then push again.

### Push was interrupted

Run `librepaper push` again. It sends the pending request first. See [Pending push](#pending-push).

### The CLI or the Action cannot sign in

The token has expired or was revoked. Create a new token in Sharing. Then replace the `LIBREPAPER_TOKEN` secret, or export the new value in your shell.

### The Action stops with "run librepaper push"

A send from an earlier run is pending. Re-run the push workflow for the failed commit, or run `librepaper push` in a clone of the repository.
