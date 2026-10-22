---
title: "Git"
---

Sync a project with a Git repository. Pull and push are buttons you press. LibrePaper never syncs on its own.

Git sync runs in the [desktop app](desktop-app.html). Browser editors see the changes a desktop pulls in.

## Before you start

You need the LibrePaper desktop app and Git 2.38 or later.

Check the Git version in a terminal.

```sh
# Print the installed Git version. It must be 2.38 or later
git --version
```

Install Git if the command is missing or too old.

- macOS: `brew install git`, or run `xcode-select --install` for the Command Line Tools.
- Windows: `winget install Git.Git`.
- Linux: use your package manager, for example `sudo apt install git`. Some distributions ship an older Git, so check the version after you install it.

## Connect

1. Open the Git panel from the Git tab in the side rail, or from "Git..." in the file menu.
2. Enter an HTTPS or SSH URL, or pick a local repository folder.
3. Pick a branch. Optionally pick a folder inside the repository. The folder holds the project's files. Everything outside it is never touched.
4. Read the first comparison. It lists the files that will come in, the files that will go out, the files that differ on both sides, and the files that will not sync. It also shows how many syncable files each top-level folder holds.
5. If files differ on both sides, choose which side keeps them: "Keep LibrePaper's copies" or "Keep the repository's copies". The choice covers only those files. Files that exist on one side only move to the other side.
6. Click "Connect" to record the connection and run the first sync. Click Cancel to change nothing on either side.

Link the folder that holds the project's files. A repository often holds code next to the paper. Link the repository root only if the project is the whole repository.

## Local folders

Pushing to a local repository folder updates the files in that folder, even when the branch is checked out there.

- The folder must be clean. Push is refused if it has uncommitted changes or untracked files that the push would overwrite. Commit or stash them first, then push again.
- Remote URLs are not affected. A checked-out branch on an SSH or HTTPS remote is still refused, and the panel suggests another branch.

## Credentials

LibrePaper uses your own Git setup. It does not ask for a password and does not store one.

HTTPS uses Git's credential helper. Check which helper is set.

```sh
# Print the credential helper Git uses. No output means none is set
git config credential.helper
```

If nothing is set, set one up for your system. The [Git credential documentation](https://git-scm.com/docs/gitcredentials) lists the options.

SSH uses your SSH agent and keys. Add your key to the agent, then check that it is listed.

```sh
# Add your key to the SSH agent, then list the keys the agent holds
ssh-add ~/.ssh/id_ed25519
ssh-add -l
```

- If Git asks for a password in a terminal, LibrePaper cannot answer it. The sync stops with an error. Set up the credential helper or the SSH agent first.
- Never put a password in the URL. LibrePaper refuses a URL that contains one.

## Pull and push

- Pull fetches the branch, merges its changes with your edits since the last sync, and applies the result to the open project. Coauthors see the text change where it changed.
- Push pulls first. Then it shows the outgoing files and a commit message. The default message names the changed files. Confirm to push one commit to the branch.
- If the branch moved while you were working, Push pulls again and asks you to confirm again.
- The commit's author is you. Its committer is your Git identity. Check the identity Git uses.

```sh
# Print the name and email Git uses for commits
git config user.name
git config user.email
```

## Conflicts

Separate edits merge automatically. Edits to the same lines are a conflict. A conflict opens the resolver. It shows three versions of the text: the base, your copy in LibrePaper, and the repository's copy. The result is editable.

Nothing changes in the project until you apply the resolution. Closing LibrePaper keeps a conflict you have not finished.

Other conflicts open the same resolver in a simpler form.

- A file deleted on one side and changed on the other: keep the changed file or delete it.
- A binary file: keep the bytes from one side.

## What syncs

A synced file is a project file under the linked folder. Its extension must be one LibrePaper supports, such as `.qmd`, `.tex`, `.typ`, `.md`, `.bib`, `.r`, `.py`, `.csv` or `.json`.

Scripts and data in supported formats are synced. A `data/` folder of CSV files is synced unless the ignore file names it.

These files are never synced:

- `.git*` files and the `.github/` folder
- `Makefile`, `.Rproj`, the `renv/` folder, `.rds` and `.parquet` files
- archives and binaries in formats LibrePaper does not support
- files outside the linked folder
- files that match `.librepaperignore`
- Git LFS pointer files, files over the size limit, and paths LibrePaper cannot hold

Unsynced files are never fetched, written or deleted. The panel lists them.

Other rules:

- A file deleted in the repository is deleted in LibrePaper if it was synced. A file deleted in LibrePaper is deleted in the repository if it was synced.
- Suggestions are not pushed. Accept or reject them in LibrePaper before you pull or push.
- A repository can hold 200 syncable files by default. A pull refuses a repository with more.
- Two paths that differ only by case or Unicode form cannot both be project files. A pull refuses and names both.

## Ignore file

`.librepaperignore` is optional. Put it where you put `.gitignore`, at the repository root or in the linked folder. It uses gitignore syntax, including `!` to bring back a path.

This two-line file leaves the `scratch/` folder and the `data/raw/` folder alone:

```text
scratch/
data/raw/
```

A matched path is unsynced in both directions. The ignore file cannot bring back files that are never synced. LibrePaper does not write this file for you.

## Line endings

- Files LibrePaper does not change keep their line endings in the repository.
- Files LibrePaper changes are written with LF line endings.

## Several computers

Each computer connects to the same repository with its own Git setup.

A local repository folder must be chosen again on each computer. The folder's path is stored only on the computer that picked it.

## Disconnecting

"Disconnect" forgets the shared connection record. The repository is not changed.

The private copy stays on your computer. It keeps unpublished commits and saved conflict sessions.
