---
title: "Git"
---

LibrePaper can keep a web project and a Git repository in step. Coauthors who prefer the browser keep writing there, and their named versions arrive in the repository as commits. Coauthors who prefer their own editor push commits, and each one appears in the project's History as a named version under their name.

Git does all the merging. When a file changed on both sides, Git's usual conflict tools take over. LibrePaper never stores a GitHub credential.

There are three ways to sync. Pull brings the latest versions from the LibrePaper server into the repository. Push sends the repository's changes to the server.

| Way | Best for |
| --- | --- |
| [The Git panel](#the-git-panel) in the desktop app | Syncing by hand, with buttons |
| [The `librepaper` command](#the-command-line) | Scripts, servers, and plain folders without Git |
| [The GitHub Action](#github-action) | Syncing automatically, with nobody's laptop involved |

## The Git panel

Open a project in the [desktop app](desktop-app.html) and choose the Git panel. Type the repository's address into the **Repository URL, SSH, or local path** field and click **Connect**. For a repository already on your computer, click **Browse…**, choose its folder, and click **Connect**. A path you type is confirmed in the folder chooser before it connects.

For a remote address, LibrePaper keeps a copy of the repository in a folder on this computer. The first time a project connects to a remote, the desktop app asks you in a dialog to allow it. That allowance covers this project and this address only.

Two buttons do the work. Each one brings in the other side's changes first, then sends its own.

- **Get repository changes** brings the repository's new commits into the project and its History.
- **Send project changes** sends the project's files to the repository's branch.

If a file changed on both sides, the panel lists it. Choose **Keep project's** or **Take repository's** for each file. When the last file is settled, the merge is committed.

With a local repository, Get and Send refuse while tracked files have uncommitted changes. Conflicts stay in the repository for your own Git tools to resolve, and only committed files are sent.

**Disconnect** asks first. For a remote, it deletes LibrePaper's copy of the repository on this computer. For a local folder, the folder is left alone. The repository and the project stay as they are either way. Each computer connects on its own.

If a transfer was interrupted, the panel offers **Retry interrupted transfer**.

In the browser, the panel shows the last change sent from a repository and asks you to open the project in the desktop app to connect it.

### Credentials

Git signs in to GitHub, GitLab or any other host with your own SSH key or credential helper, just as it does on the command line. If sign-in fails, the panel shows the steps for your host and system.

On GitHub, the simplest setup is the GitHub CLI, which can create and upload an SSH key for you:

```sh
gh auth login   # choose SSH when asked for the Git protocol
```

Then connect with the SSH address, such as `git@github.com:owner/repo.git`, found on GitHub under Code → SSH. An `https://` address also works once `gh auth setup-git` has set Git's credential helper. The panel's sign-in help shows that command.

## The command line

The `librepaper` command comes with the desktop app's Homebrew, Scoop, Windows installer, `.deb`, `.rpm` and Nix packages. The plain macOS `.dmg` does not add it to your PATH. On a server, [install the command alone](cli.html#install).

### Sign in

Run `librepaper login` once to sign in on this computer for 90 days. On a server or in CI, use a project token instead:

1. In the project, open the Agent panel (the robot icon).
2. Open the Connect tab, then click **New token**.
3. Choose the **Edit** permission and set **Expires** to 180 days or 1 year. Edit tokens default to one day.
4. Copy the token from "Command line and MCP configuration".

See [Agent tokens](agents.html#agent-tokens).

Pass the token with `--token`, or set it for the shell without leaving it in your history:

```sh
read -rs LIBREPAPER_TOKEN && export LIBREPAPER_TOKEN
```

A project token works only for its own project. Creating a new project with `push --title` needs `librepaper login`.

### Pull and push

The first pull takes the project's address and the branch to sync. The repository remembers both, so later commands need neither.

```sh
librepaper pull https://app.librepaper.org/docs/<slug> --branch main
```

From then on, the everyday loop is:

```sh
librepaper pull     # bring web edits into your branch
librepaper push     # send your commits to the project
```

Pull fetches each named version as a commit on `refs/librepaper/main`, which is a ref LibrePaper keeps, not a branch you check out. It adds the project's current state as one more commit if it has unnamed edits, then merges the result into your branch. If the merge conflicts, resolve it the usual Git way and commit. Pull refuses to start while tracked project files have uncommitted changes. `--no-merge` fetches without merging, and skips that check.

Push sends every new, changed and deleted file in one request. The project accepts all of them or none. The version takes its name from your latest commit subject, followed by "and N more" if several commits went out together. It is credited to that commit's author. Override these with `--message` and `--author`. Push is refused if a file you changed was also changed on the web since your last pull. Pull, then push again.

A text file renamed with its text unchanged is sent as a rename, so its comments stay on it.

To create a project from an existing repository, run `librepaper push --title "Thesis draft"`. To see what a first pull would do without writing anything, add `--preview`. Preview works only before the folder is linked.

### One branch, one project

A repository syncs one branch with one project. The commands only run on that branch, and they never switch branches for you. Other branches are yours for experiments: merge them into the synced branch with Git when they are ready. A repository linked to one project cannot be pushed to another.

### Plain folders

The commands also work in a folder that is not a Git repository. Pull writes the project's files into the folder directly. Files that changed only on the web are updated, files that changed only in the folder are kept, and files deleted on the web go to the trash. When a file changed on both sides, pull lists it and leaves it alone. Run pull again with `--mine PATH` to keep your copy, or `--theirs PATH` to take the web's. In a plain folder, `push` needs `--message`.

### If a push is interrupted

Run `librepaper push` again. Push saves the request before it sends it, so the retry finishes the earlier send first and then sends anything new. Until it succeeds, `pull` refuses to start.

## GitHub Action

The [`LibrePaper/sync-action`](https://github.com/LibrePaper/sync-action) runs pull and push on GitHub. Every push to your branch reaches the project at once. Web edits reach the repository on a schedule.

1. Create a project token, as described in [Sign in](#sign-in). Choose the **Edit** permission and a longer expiry, since Edit tokens default to one day. The Action stops when its token expires. In the repository, add the token as a secret named `LIBREPAPER_TOKEN` (Settings → Secrets and variables → Actions).
2. Save this workflow as `.github/workflows/librepaper.yml`, with your project's address:

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

Keep `branch` equal to the `ref` that `actions/checkout` uses. The first run links the repository and the project, bringing each side's files to the other. If a file differs on both sides, that first run stops, and someone merges it once on a computer.

Hourly is a good default. A run with nothing new costs one small request. On a private repository, hourly uses about 730 of GitHub Free's 2,000 monthly minutes. Every 15 minutes would exceed them. Public repositories run free. To sync immediately, click **Run workflow** in the Actions tab. To sync only during working hours, use a cron line such as `"*/30 8-20 * * 1-5"` (times are UTC).

The Action never merges text or force-pushes. When both sides changed a file, it commits the web's version to a `librepaper/sync` branch and opens a pull request titled "LibrePaper changed files you also changed". The run then fails on purpose so it shows red. Merge the pull request, and the next push sends the result to the project.

Anyone who can push to the repository can change the project this way. Each change is a named version under the Git author, visible and restorable in History. To stop the Action, delete its token in the Agent panel.

## What syncs

Files sync if the project accepts their extension. That covers sources such as `.tex`, `.typ`, `.qmd`, `.md`, `.bib` and `.csv`, and figures and fonts such as `.png`, `.svg`, `.pdf` and `.woff2`. Text files must be UTF-8, and line endings are kept as they are.

These stay out:

- files your `.gitignore` excludes, and hidden files such as `.gitignore` itself;
- files you mark with the `-librepaper` attribute in `.gitattributes`, such as `notes.tex -librepaper`;
- build outputs the project does not accept, such as `.aux` and `.log`. A rendered PDF is accepted as a figure, so add it to `.gitignore` to keep it out;
- comments, suggestions and chat, which live only in the web project. Pending suggestions are left out of pulled text.

A project holds up to 200 files. Your server may set other limits.

For a Quarto project, the repository can also be the [Run directory](notebooks/quarto.html#git-tutorial) your code runs in. Scripts such as `.R` and `.py` are not accepted by the project, so they stay in the repository.

### Versions and commits

Each named version becomes one commit. The commit is authored by the person who saved the version, with an address made from their name at the site's host. It is dated when the version was saved and titled with the version's name. A `LibrePaper-Version` trailer records which version it was. Each push becomes one named version, and it is not turned back into a second commit when you pull. If the project's history was trimmed past your last pull, pull starts again from the current files as one new commit. The commits you already have stay as they are.

## Troubleshooting

| Message or symptom | What to do |
| --- | --- |
| Pull refuses: uncommitted changes | Commit, then pull again. |
| Pull refuses: a push is pending | Run `librepaper push` first. |
| Push says an earlier push was sent under a different sign-in | Sign in as the same account, then run `librepaper push` again. |
| Wrong branch, or detached HEAD | `git switch` to the synced branch. |
| No branch set | Run pull once with `--branch BRANCH`. |
| Push says "Pull first." | Someone edited the same file on the web. Pull, resolve, push. |
| Push says a suggestion is pending | Accept or reject the suggestion on the web, then push. |
| Panel says a transfer was interrupted | Choose **Retry interrupted transfer**, then Get or Send again. |
| Sign-in fails | The token expired or was revoked. Create a new one and update `LIBREPAPER_TOKEN`. |
| The Action says a send from an earlier run is still pending | Re-run the push workflow, or run `librepaper push` in a clone. |
