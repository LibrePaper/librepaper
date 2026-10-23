---
title: "Git"
---

Git sync keeps a project in LibrePaper and in one branch of a Git repository. The project can sync with the whole repository, or with one folder inside it. Pull and push are buttons that you press, unless you turn on [automatic sync](#automatic-sync) on one computer. Coauthors who work in the browser see the changes that a desktop app pulls in.

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
2. Enter an HTTPS or SSH URL, or press "Choose a local repository".
3. Pick a branch. Optionally enter a folder inside the repository. That folder holds the project's files. Everything outside it is never touched. Leave it empty for the whole repository.
4. Click "Connect". This records the connection and nothing else. No file changes on either side.
5. Press "Pull" to run the first sync.

Until the first sync, LibrePaper has no earlier state to compare with. Remote changes lists every file on the branch, and Local changes lists every file in the project, even files that are the same on both sides. The first pull sorts them out.

- Files that are the same on both sides match. Nothing changes.
- A file only on the branch comes into the project.
- A file only in the project stays. It is a local change, which you can push.
- A file that differs on both sides opens the resolver. See [Conflicts](#conflicts).

After the first sync, the two lists show only real changes.

Link the folder that holds the project's files. A repository often holds code next to the paper. Link the repository root only when the project is the whole repository.

## Daily use

### The Git panel

The panel has four parts.

- The repository header shows the repository's name and, below it, the branch and the folder that the project syncs. Click the name to open the repository menu. See [The repository menu](#the-repository-menu).
- The status row says where sync stands. The panel checks the remote when it opens. "Retry" checks again.
- **Remote changes** lists the files that changed on the branch since the last sync, with a "Pull" button. It appears only when the remote has new commits.
- **Local changes** lists the files that differ from the last sync. The list updates as you edit. The refresh button recounts it.

Click a file in either list to see its changes. See [Compare a file](#compare-a-file).

The status row can show these states.

- **Checking the remote.** The panel is reading the remote.
- **Up to date.** The remote has no new commits. The panel shows the time of the check.
- **New commits on the remote.** Press "Pull" in Remote changes to bring them in.
- **Pulling.** A pull is running.
- **Conflicts need resolution.** A conflict was saved. Press "Resolve" to open it.
- **A sync was interrupted.** Press "Resume" to finish it.
- **Git could not sign in.** See [When sign-in fails](#when-sign-in-fails).
- **Resolve the conflicts in the editor area.** The resolver is open.

With automatic sync on, a line under the status row says when the project last pulled and pushed, or why it paused.

### The repository menu

Click the repository's name to open the menu. It shows the branch, who connected the project, and when it last pulled and pushed. Hover over a time to see the full date and who did it. It also offers these actions.

- **Open in browser.** Opens the repository on GitHub, GitLab, Codeberg, Bitbucket or SourceHut.
- **Copy remote URL.** Copies the repository's address.
- **Switch branch...** Stops syncing with this branch, then lets you connect another branch of the same repository. Files stay as they are in LibrePaper.
- **Push to branch...** Pushes your local changes to a new branch. See [Push to a branch](#push-to-a-branch).
- **Disconnect...** Asks for confirmation first. See [Disconnecting](#disconnecting).

### Compare a file

Click a file in Remote changes or Local changes. Its diff opens in the editor area, and the Git panel stays open beside it.

- The diff compares LibrePaper's copy with the latest commit on the branch.
- Lines marked − are the repository's version. Lines marked + are LibrePaper's version.
- Click another file in the panel to compare it instead. "Refresh" compares again. "Close" returns to the editor.

The comparison only reads. It changes nothing in the project or in the repository.

### Pull

Pull fetches the branch. It merges the branch's changes with your edits since the last sync, and applies the result to the open project. Coauthors see the text change where it changed.

### Commit and push

Commit & push commits your local changes as one commit and pushes it to the branch. It never pulls. When the remote has new commits, the button is off and the panel says "Pull before pushing". Pull first, then push.

The commit message defaults to a message that names the changed files. Type your own message to replace it.

Commit & push stops in two cases.

- The list of files changed since you last saw it. Check the list, then press Commit & push again. Only a list that you have seen is pushed.
- The branch moved while you were working. Press Commit & push again. If the remote now has new commits, pull first.

The commit's author is you. Its committer is the Git identity on your computer. To see the identity that Git uses, run these commands.

```sh
# Print the name that Git uses for commits
git config user.name

# Print the email address that Git uses for commits
git config user.email
```

### Push to a branch

Push to branch sends your local changes to a new branch of the repository, for example to open a pull request. The project stays on its own branch.

1. Click the repository's name and choose "Push to branch...". It is available when you have local changes and the remote has no new commits.
2. Type a name for the new branch, for example `review/intro`.
3. Press "Push". LibrePaper commits your local changes as one commit, with the commit message from the panel if you typed one, and pushes it to the new branch.
4. On GitHub, the confirmation has an "Open pull request" button.

After the push, the project is still connected to its own branch. Your edits still show as local changes, because that branch does not have them yet. When the pull request is merged, pull. The edits then match the branch and leave the list.

Push to branch only creates a branch. It never replaces one.

- If a branch with that name exists on the remote, the push stops and asks for another name.
- To push more changes later, push to another new branch, or merge the pull request and push to your branch as usual.
- To keep working on the new branch, use "Switch branch..." and connect to it.

### Automatic sync

Automatic sync keeps the project in step with the branch while its window is open. Turn it on with the switch "Sync automatically while this window is open" in the Git panel. The switch is remembered on that computer only, for that connection. Disconnecting or switching branch turns it off.

While the switch is on, the app does the following.

- Checks the remote every two minutes. It also checks when the window comes back into focus, or when the network comes back.
- Pulls new commits on the branch at once.
- Pushes edits made in LibrePaper, by anyone, as one commit. It waits until nobody has typed for three minutes. It pushes at most once every ten minutes, and always within thirty minutes of the first edit that is not pushed yet.
- Never force-pushes. If the branch has moved, it pulls first and then pushes.

If a conflict stops the sync, it changes nothing. The system shows a notification, "Git sync paused". The Git panel shows "Conflicts need resolution" with a "Resolve" button. After you resolve the conflict, automatic sync continues on its own.

If Git cannot sign in, or another problem stops the sync, it pauses and shows the reason with a "Retry" button. A network failure makes the app try again later, and each retry waits longer than the last.

Automatic sync runs while the project's window is open. It also runs while the window is hidden in the tray after you close it. Quitting LibrePaper stops it, and it starts again when the project opens. On a Linux desktop without tray support, closing the window quits the app.

If you work in Git, push to the branch from your own clone as usual. Browser coauthors see the change after the next check.

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

Each computer connects to the same repository with its own Git setup. Turn on automatic sync on one computer only.

A local repository folder must be chosen again on each computer. The folder's path is stored only on the computer that chose it.

## Disconnecting

Click the repository's name and choose "Disconnect...". The panel asks you to confirm. Disconnecting forgets the shared connection record. The repository is not changed. Sync stops until you connect again.

The private copy stays on your computer. It keeps unpublished commits and any saved conflict sessions.

## Safety

These rules protect your repository.

- A push is fast-forward only. It adds one commit on top of the remote head that LibrePaper last saw. If the remote has moved, the push stops.
- Push to branch only creates a new branch. It never moves or replaces a branch that exists.
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

### The list changed

The list of files changed since you last saw it, so Commit & push stopped. Check the list. Then press Commit & push again.
