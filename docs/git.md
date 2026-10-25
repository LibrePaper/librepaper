---
title: "Git"
---

Two commands in the `librepaper` CLI move files between a folder and a web project. `librepaper pull` brings the project's named versions into the folder. `librepaper push` sends the folder's changes back. Git is yours. LibrePaper never merges text, and it never stores a credential for GitHub. A GitHub Action runs the two commands, so a repository and a web project stay in step with nobody's laptop involved.

## In the app

The **Git** panel is the guided way to do what the rest of this page describes by hand. It appears in a web project for its editors and owners, and in a folder project in the desktop app.

### In a web project

The panel shows:

- **Last push.** The newest version pushed from Git, such as "Last pushed by Alice, 2 hours ago: Reword the greeting", or "No push yet".
- **Token.** The owner sees the project's tokens with the Edit role. **Create token for a repository** makes a token named GitHub Action that expires in 365 days, and shows it once with a Copy button. Revoke removes a token. Other editors see the list and a note that only the owner can create one.
- **Workflow.** The workflow from the [GitHub Action](#github-action) section, with your project's address filled in, and a Copy button. Save it as `.github/workflows/librepaper.yml`.
- **Before the first run.** The seeding commands from the same section, with your address filled in, and the note about unrelated histories.
- **Clone.** The `librepaper pull <address>` line, for a clone on this computer alone.

### In a folder project

The panel has two sections.

- **LibrePaper.** Sign in, then Link the folder to a web project or Publish it as a new one. Pull and Push move changes, and Unlink removes the link.
- **Repository.** Without a `.git` folder, **Initialize** runs `git init -b main`. With one, the section shows:
  - the branch, which you can switch or create, and the remote, which you can set;
  - **Changes**, the files git reports. **Diff** shows a file against `HEAD` in the editor area;
  - a message and **Commit**, which stages every file unless you untick it;
  - **Pull** and **Push**, with the ahead and behind counts after a fetch;
  - **Sync**, which commits with the message "Edits from LibrePaper" when there are changes, then pulls and pushes. It stops at the first step that fails and says which one.

A pull that conflicts leaves git's conflict markers in the files. The panel lists the conflicted files. Resolve them in the editor, then Commit to finish the merge. Git 2.38 or later must be installed. Without it, the section says so.

On the landing page, **Clone a repository…** beside **Open folder…** clones a Git URL into a subfolder of a folder you choose, and opens the clone as a folder project.

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
# First run: give the project's address. The folder remembers it
librepaper pull https://app.librepaper.org/docs/<slug>

# Send this folder's changes to the project
librepaper push --message "Tighten the abstract"
```

### pull

```sh
# In a repository: fetch the named versions and merge them
librepaper pull

# In a repository: fetch only. Merge with git later
librepaper pull --no-merge

# Outside a repository: keep your copy of a file that changed on both sides
librepaper pull --mine intro.tex
```

- In a repository, pull fetches each named version as a commit on `librepaper/main`. Then it merges that into your branch.
- If the merge conflicts, git leaves the conflict for you. Resolve it with your tools.
- In a repository, pull refuses to start with uncommitted changes. Commit first.
- Outside a repository, pull writes the newest version into the folder. A file that changed only on the web is written. A file that changed only in the folder is kept. A file deleted on the web goes to the trash.
- A file that changed on both sides is not written. Pull lists it. Run pull again with `--mine PATH` to keep your copy, or `--theirs PATH` to take the web copy.
- A file the folder itself excludes (ignored by `.gitignore`, hidden, or not a file the project can hold) is left alone even when the web has it. Pull lists those too.

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
- `--token TOKEN` gives the token for one run. `LIBREPAPER_TOKEN` works too.

## GitHub Action

The Action runs `pull` and `push` for a repository on GitHub. Nothing has to run on your laptop.

1. Add a project token as a repository secret named `LIBREPAPER_TOKEN`. Use Settings, then Secrets and variables, then Actions. A token for one project keeps the Action away from every other project.
2. **Before the first run,** seed the LibrePaper state once. Run these in a clone of the repository on your computer.

```sh
# Once, on your computer, in a clone of the repository
librepaper pull https://app.librepaper.org/docs/<slug>   # links the clone and fetches the project as commits
git merge --allow-unrelated-histories librepaper/main    # only when the repository and the project share no history
git push origin main 'refs/librepaper/*'                 # the Action starts from this state
```

A repository that started as a web project (`librepaper pull` into an empty repository) or a project that started as a repository (`librepaper push --title`) already share history, so the merge line is not needed. A repository and a project that grew separately need it once.

The Action refuses to run until `refs/librepaper/state` exists on GitHub, and after that it keeps that state current itself.

3. Save this workflow as `.github/workflows/librepaper.yml`. Put your project's address in `project`.

```yaml
on:
  push:
    branches: [main]
  schedule:
    - cron: "*/15 * * * *"
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

The Action takes two inputs.

- `project` is the project's address, `https://<site>/docs/<slug>`.
- `token` is the secret. The workflow passes `LIBREPAPER_TOKEN` to it.

GitHub gives each run its own `GITHUB_TOKEN`. The `permissions` line lets it write to the repository and open pull requests. You do not add it.

What happens:

- **On a push to `main`**, the Action pushes the new commit to the project. The commit's subject names the version, and its author becomes the version's author.
- **On a schedule**, every 15 minutes, the Action pulls. A clean merge is pushed to `main`. When nothing is new, the run ends with no commit.
- **When a push is refused**, the Action commits the web's files to a branch named `librepaper/sync` and opens one pull request titled "LibrePaper changed files you also changed". The run then fails on purpose, so it shows red. Merge the pull request in git. The next push to `main` sends the merged files.
- **When a scheduled pull conflicts**, the Action aborts the merge and does the same.
- **When `main` moved during a scheduled run**, the push to `main` fails, the LibrePaper state is not published, and the next run repeats the pull from the same point.
- **When an older push run is started again**, the Action sees that its checkout is no longer the tip of the branch and pushes nothing, so an old commit never rolls the project back.
- **On every run that ends well**, the Action pushes the LibrePaper refs back to GitHub. The next run, on a new machine, starts from the same state.

Anyone with push rights on the repository can change the project this way. Each change is a named version under the git author. You can see it and restore it in History. To stop the Action, revoke its token in Sharing.

## Common workflows

1. **Bob and Carol write on the web.** They edit and comment in the browser, and nothing changes for them. Their named versions become commits in the repository.
2. **Alice backs up a web project.** She pulls the project into a new repository with her token, then pushes the repository and its LibrePaper refs to GitHub (`git push origin main 'refs/librepaper/*'`) and adds the workflow. From then on, the Action adds each new named version as a commit. She can also go the other way. `librepaper push --title` in a repository that holds her paper creates the project.
3. **Alice works in VS Code, and Bob works in the browser.** Alice adds a token. She runs `librepaper pull` once in her clone and pushes the LibrePaper refs, then adds the workflow. If her repository and the project share no history, she also merges them once, as the Before the first run step shows. She can also do the whole setup from the Git panel of the web project. Her pushes reach Bob within a minute, as versions in his history under her name. Bob's named versions reach her on her next `git pull`, as commits by Bob. If they both changed a file, the Action's push is refused. A pull request from `librepaper/sync` opens, and Alice merges it in git. Bob sees none of this.
4. **Charlie works on GitHub only.** He works in the same repository, with the same Action. He has nothing new to learn.
5. **Dana works in a folder with no Git.** In the desktop app she opens her web project and chooses Work in a folder from the File menu, which puts the project's files in a folder of her choice and opens it. The Git panel asks her to sign in once. She edits the folder offline with local TeX and presses Push in the Git panel. When a file also changed on the web, Pull asks her which copy to keep, file by file.

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
- History. Only named versions reach Git, as commits.
- Build outputs, such as `.aux` and `.log`. They stay on your computer. A file a local render wrote into the folder, such as the PDF, is neither imported into the project nor synced.

## Versions and commits

- In a repository, each named version becomes one commit when you pull. The commit's author is the person who saved the version, shown as `handle@site`. Its date is when the version was saved. Its message is the version's name.
- Each commit has a `LibrePaper-Version` trailer that names the version.
- Renders and agent edits are unnamed, so they never become commits.
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

The repository has uncommitted changes. Commit them, then pull again.

### Push says "Pull first."

Someone changed a file on the web. Pull, resolve any conflict, then push again.

### Push says a suggestion is pending

Accept or reject the suggestion on the web. Then push again.

### Git refuses the first merge

The repository and the project share no commits. This happens when you link an existing repository to an existing project. Allow the merge once.

```sh
# Allow one merge between histories that share no commits
git merge --allow-unrelated-histories librepaper/main
```

### The CLI or the Action cannot sign in

The token has expired or was revoked. Create a new token in Sharing. Then replace the `LIBREPAPER_TOKEN` secret, or export the new value in your shell.
