---
title: "Git"
---

Git sync keeps a project in LibrePaper and in one branch of a Git repository. The project can sync with the whole repository, or with one folder inside it. Pull and push are buttons that you press. Nothing syncs on its own. Coauthors who work in the browser see the changes that a desktop app pulls in.

## How it works

The desktop app runs your own Git. It keeps a private copy of the repository on your computer.

- LibrePaper never writes into your repository except through a normal `git push`.
- Every push adds one commit on top of the branch. A push never rewrites history.
- Files that LibrePaper does not sync are left untouched.

## Requirements

You need these before you connect a project.

- The [LibrePaper desktop app](desktop-app.html). Git sync is not available in the browser.
- Git 2.38 or later.
- A repository that you can push to.
- A way for Git to sign in without a terminal prompt. See [Credentials](#credentials).

Check the Git version in a terminal.

```sh
# Print the installed Git version. It must be 2.38 or later
git --version
```

Install Git if the command is missing or too old. Run the command for your system.

```sh
# macOS: install Git with Homebrew
brew install git

# macOS: or install the Command Line Tools, which include Git
xcode-select --install

# Windows: install Git with winget
winget install Git.Git

# Debian or Ubuntu: install Git with apt. Other Linux systems use their own package manager
sudo apt install git
```

Some Linux distributions ship an older Git. Check the version again after you install it.

## Credentials

LibrePaper uses your own Git setup. It never asks for or stores a password. Git cannot ask for a password from inside the app. Sign in once from a terminal, and Git keeps that sign-in after that.

### Which method to use

| Repository | Recommended method |
| --- | --- |
| GitHub | The GitHub CLI over HTTPS, or an SSH key |
| GitLab, Codeberg or another host | An SSH key, or HTTPS with Git Credential Manager (Windows and Linux) or the macOS keychain, and an access token |
| A local folder | Nothing. Choose the folder in the Git panel |

### GitHub with the GitHub CLI

GitHub does not accept account passwords for Git. Use the GitHub CLI to sign in.

- Install the [GitHub CLI](https://cli.github.com).
- Sign in once. Then let Git use that sign-in.

```sh
# Sign in to GitHub once. Choose HTTPS when asked
gh auth login

# Let Git use the GitHub CLI's sign-in
gh auth setup-git
```

### Other hosts over HTTPS

Sign in once from a terminal. The `git ls-remote` command reads the remote and changes nothing. Replace the URL with your repository's HTTPS address.

- Windows: Git Credential Manager comes with Git for Windows.
- macOS: Save the sign-in in the keychain. Use an access token from the host as the password.
- Linux: Install [Git Credential Manager](https://github.com/git-ecosystem/git-credential-manager). It stores the sign-in in the system keyring.

```sh
# macOS only: store the sign-in in the keychain
git config --global credential.helper osxkeychain

# Sign in once. Replace the URL with your repository's HTTPS address
git ls-remote https://example.com/owner/repo.git
```

### SSH keys

SSH uses your SSH agent and your keys. Add your key to the agent, then check that the agent lists it.

```sh
# Add your key to the SSH agent
ssh-add ~/.ssh/id_ed25519

# List the keys that the agent holds
ssh-add -l
```

On macOS, you can keep the key's passphrase in the keychain.

```sh
# macOS only: add the key and keep its passphrase in the keychain
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
```

Test the key with GitHub. Answer yes if SSH asks you to trust the host.

```sh
# Check that GitHub accepts the key
ssh -T git@github.com
```

A key with a passphrase must be in the agent. Git cannot ask for the passphrase from the app.

### When sign-in fails

If Git cannot sign in, the Git panel shows a title and steps for your host and system. The steps name the commands to run. Follow them in a terminal, then press "Try again" in the panel. The panel checks the remote again.

Two rules apply to every method.

- If Git asks for a password in a terminal, LibrePaper cannot answer it. The sync stops with an error. Set up the credential helper or the SSH agent first.
- Never put a password in the URL. LibrePaper refuses a URL that contains one.

## Connect a project

1. Open the Git panel from the Git tab in the side rail, or from "Git..." in the file menu.
2. Enter an HTTPS or SSH URL, or choose a local repository folder.
3. Pick a branch. Optionally pick a folder inside the repository. That folder holds the project's files. Everything outside it is never touched.
4. Read the first comparison. It lists the files that will come in, the files that will go out, and the files that differ on both sides. It also lists the files that will not sync, and the number of syncable files in each top-level folder.
5. If files differ on both sides, choose which side keeps them. Choose "Keep LibrePaper's copies" or "Keep the repository's copies". The choice covers only those files. Files that exist on one side only move to the other side.
6. Click "Connect" to record the connection and run the first sync. Click "Cancel" to change nothing on either side.

Link the folder that holds the project's files. A repository often holds code next to the paper. Link the repository root only when the project is the whole repository.

## Daily use

### The Git panel

The panel has three parts.

- The repository line shows the branch and the folder that the project syncs. The "..." menu opens the connection details, where you can copy the remote address or disconnect.
- The status row says where sync stands. The panel checks the remote when it opens. "Retry" checks again.
- The local changes list shows the files that differ from the last sync. The list updates as you edit. The refresh button recounts it.

The status row can show these states.

- **Checking the remote.** The panel is reading the remote.
- **Up to date.** The remote has no new commits. The panel shows the time of the check.
- **New commits on the remote.** Press "Pull" to bring them in.
- **Pulling.** A pull is running.
- **Conflicts need resolution.** A conflict was saved. Press "Resolve" to open it.
- **A sync was interrupted.** Press "Resume" to finish it.
- **Git could not sign in.** See [When sign-in fails](#when-sign-in-fails).
- **Resolve the conflicts in the editor area.** The resolver is open.

The connection details show the remote address, who connected the project, and when it last pulled and pushed.

Disconnecting asks for confirmation first. See [Disconnecting](#disconnecting).

### Pull

Pull fetches the branch. It merges the branch's changes with your edits since the last sync, and applies the result to the open project. Coauthors see the text change where it changed.

### Commit and push

Commit & push does three things in order.

1. It pulls new commits from the branch.
2. It commits your local changes.
3. It pushes that commit.

The commit message defaults to a message that names the changed files. Type your own message to replace it.

Commit & push stops in two cases.

- The pull changed the list of files. Check the list, then press Commit & push again. Only a list that you have seen is pushed.
- The branch moved while you were working. Press Commit & push again.

The commit's author is you. Its committer is the Git identity on your computer. To see the identity that Git uses, run these commands.

```sh
# Print the name that Git uses for commits
git config user.name

# Print the email address that Git uses for commits
git config user.email
```

## Conflicts

Separate edits merge automatically. Edits to the same lines are a conflict. A conflict opens the resolver in the editor area, across the source and the preview.

- The repository's copy is on the left.
- The result is on the right. It starts from your copy in LibrePaper.
- Words that differ are highlighted. The arrow beside a difference copies the repository's version into the result.
- You can type in the result. You can also choose "Use LibrePaper's" or "Use the repository's" for the whole file.
- "Close" keeps your choices. "Resume" in the Git panel opens them again.

Nothing changes in the project until you apply the resolution. Closing LibrePaper keeps a conflict that you have not finished.

Other conflicts open the same resolver in a simpler form.

- A file was deleted on one side and changed on the other. Keep the changed file, or delete it.
- A binary file. Keep the bytes from one side.

## What syncs

A synced file is a project file under the linked folder. Its extension must be one that LibrePaper supports. Examples include `.qmd`, `.tex`, `.typ`, `.md`, `.bib`, `.r`, `.py`, `.csv` and `.json`.

Scripts and data in supported formats sync. For example, a `data/` folder of CSV files syncs unless the ignore file names it.

These files never sync.

- Files whose names start with `.git`, and the `.github/` folder
- `Makefile`, `.Rproj` files, the `renv/` folder, and `.rds` and `.parquet` files
- Archives and binaries in formats that LibrePaper does not support
- Files outside the linked folder
- Files that match `.librepaperignore`
- Git LFS pointer files, files over the size limit, and paths that LibrePaper cannot hold

Unsynced files are never fetched, written or deleted. The panel lists them.

Some other rules apply.

- A file deleted in the repository is deleted in LibrePaper, if it was synced. A file deleted in LibrePaper is deleted in the repository, if it was synced.
- Suggestions are not pushed. Accept or reject them in LibrePaper before you pull or push.
- A repository can hold 200 syncable files by default. A pull refuses a repository with more.
- Two paths that differ only by case or Unicode form cannot both be project files. A pull refuses and names both.

### Ignore file

`.librepaperignore` is optional. Put it where you put `.gitignore`, at the repository root or in the linked folder. It uses gitignore syntax, including `!` to bring back a path.

This file leaves the `scratch/` folder and the `data/raw/` folder alone.

```text
scratch/
data/raw/
```

A matched path is unsynced in both directions. The ignore file cannot bring back files that are never synced. LibrePaper does not write this file for you.

### Line endings

- Files that LibrePaper does not change keep their line endings in the repository.
- Files that LibrePaper changes are written with LF line endings.

## Local folders

You can sync with a local repository folder instead of a remote URL. Pushing to a local folder updates the files in that folder, even when the branch is checked out there.

- The folder must be clean. A push is refused if the folder has uncommitted changes, or untracked files that the push would overwrite. Commit or stash those changes, then push again.
- A checked-out branch on an SSH or HTTPS remote is still refused. The panel suggests another branch.

## Several computers

Each computer connects to the same repository with its own Git setup.

A local repository folder must be chosen again on each computer. The folder's path is stored only on the computer that chose it.

## Disconnecting

Open the panel's "..." menu and choose "Disconnect". The panel asks you to confirm. Disconnecting forgets the shared connection record. The repository is not changed. Sync stops until you connect again.

The private copy stays on your computer. It keeps unpublished commits and any saved conflict sessions.

## Safety

These rules protect your repository.

- A push is fast-forward only. It adds one commit on top of the remote head that LibrePaper last saw. If the remote has moved, the push stops.
- The list of changes that you saw is the list that gets pushed.
- Files that LibrePaper does not sync are never touched.
- Your Git hooks do not run on LibrePaper's pushes. Checks such as secret scanning in a `pre-push` hook are skipped. Run those checks in your continuous integration instead.
- To undo a LibrePaper commit, make a new commit that reverts it. Do not rewrite the branch.

```sh
# Replace the hash with the commit that LibrePaper made
git revert <commit-hash>
```

## Troubleshooting

### Git could not sign in

The panel shows the steps for your host and system. Follow them, then press "Try again". See [When sign-in fails](#when-sign-in-fails).

### Git 2.38 or later is needed

The panel reports the Git version that it found. Install Git 2.38 or later, then check the version in a terminal. See [Requirements](#requirements).

### The remote refused the push because the branch is checked out

The remote refused the push because that branch is checked out there. This happens with a repository on a server that is not bare. Connect to another branch instead.

### The folder has uncommitted changes

A push into a local folder is refused while that folder has uncommitted changes. Commit or stash them in the folder, then push again.

### The commit this project last synced with is no longer on the remote

The remote no longer has the commit that LibrePaper last synced with. Connect again from the Git panel to compare the files.

### A sync was interrupted

The status row says "A sync was interrupted". Press "Resume" to finish it. If Resume cannot finish the sync, the panel says so. Try again later.

### These files would have the same name in LibrePaper

LibrePaper ignores case and accents in file names, so `Notes.md` and `notes.md` are the same file to it. Git can hold both. When a pull or a conflict resolution would leave both in the project, LibrePaper stops and changes nothing. Rename one of the files, in LibrePaper or in the repository, then try again. In the resolver, another choice for that file also works.

### The list changed after pulling

The pull changed the list of files, so Commit & push stopped. Check the list. Then press Commit & push again.
