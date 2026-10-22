---
title: "GitHub"
---

Connect a project to a branch or folder on GitHub. Then pull changes in and push changes out. Each sync is a button you press. LibrePaper never syncs on its own.

## Connect

1. Open the project menu and choose "Connect to GitHub". Any editor can connect.
2. A popup opens GitHub's consent page for LibrePaper. Approve it, and the popup closes.
3. Pick a repository you can write to, a branch, and optionally a folder inside it.
4. Read the first comparison. It lists the files that will come in, the files that will go out, the files that differ on both sides, and how many syncable files each top-level folder holds.
5. Click "Pull" or "Push" to make the first sync, or "Cancel".

The consent click is the only sign-in step. The server does not store your GitHub token. Your browser keeps it on your machine, and sign-out clears it.

If the project is not empty and some files differ on both sides, "Pull" is refused. "Push" overwrites the repository's copies. The comparison says so before you click.

## Linking a folder

A repository often holds code next to the paper. Link the folder that holds the project's files. Everything outside that folder is ignored.

Link the repository root if the project is the whole repository.

## What is synced

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

- A file deleted on GitHub is deleted in LibrePaper if it was synced. A file deleted in LibrePaper is deleted on GitHub if it was synced.
- Suggestions are not pushed. Accept or reject them in LibrePaper before you pull or push.
- Project text is saved with LF line endings. A CRLF file on GitHub becomes LF on the next pull. The next push then shows that file once as a whole-file change on GitHub.
- A repository can hold 200 syncable files by default. A pull refuses a repository with more.
- Two paths that differ only by case or Unicode form cannot both be project files. A pull refuses and names both.

## The ignore file

`.librepaperignore` is optional. Put it where you put `.gitignore`, at the repository root or in the linked folder. It uses gitignore syntax, including `!` to bring back a path.

This two-line file leaves the `renv/` folder and the `data/raw/` folder alone:

```text
renv/
data/raw/
```

A matched path is unsynced in both directions. The ignore file cannot bring back files that are never synced. LibrePaper does not write this file for you.

## Pull

Pull brings GitHub's changes into the open project. Coauthors see the text change where it changed. Comments on text that the pull does not touch stay where they are.

- Pull applies every change or none.
- A pull that finds a conflict refuses. See Conflicts below.
- A pull that lands on a pending suggestion refuses. Accept or reject the suggestion, then pull again.

## Push

Push writes the changed synced files as one commit on the branch. It asks for a commit message. The default lists the names of the changed files.

The commit is made by the account that connected the project. Its author is the last LibrePaper editor.

If the branch moved on GitHub since your last sync, Push pulls first and then pushes. If that pull refuses, the push stops and the panel says why.

## Conflicts

A conflict is a synced file that changed on GitHub and in LibrePaper since the last sync, with different content. A pull with a conflict applies nothing. It lists the files.

To resolve a conflict:

1. Click "Create a branch with my changes". LibrePaper creates a branch named `librepaper/<date>`. The branch starts from the last synced commit and holds your changes to synced files.
2. Open the GitHub compare link shown in the panel. It opens a pull request from that branch into the connected branch.
3. Merge the pull request on GitHub, resolving conflicts in GitHub's editor. You can also merge locally with git.
4. Back in LibrePaper, pull again. The pull finishes the job. It applies the merged files and clears the conflict.

If you change a conflicting file after creating the branch, the next pull reports it as a conflict again. Create the branch again to replace the old one. The old branch stays on GitHub.

## Disconnecting and reconnecting

"Disconnect" forgets the link and the record of what was synced. The repository is not changed.

If your GitHub access ends, the next pull or push fails with "Reconnect with your account". Any editor can reconnect with their own consent. The link and the sync record are kept.

## Self-hosting

A self-hosted LibrePaper uses its own GitHub App. This is separate from the sign-in app under `[auth.github]`.

1. Create a GitHub App at `https://github.com/settings/apps/new`, or under your organization.
2. Under repository permissions, set Contents to read and write, and Metadata to read.
3. Turn on "Expire user authorization tokens".
4. Set the callback URL to `https://paper.example/api/forge/github/callback`. Replace `paper.example` with your app origin, the value of `origins.app`.
5. Note the app's slug, which is the last part of its public URL, and its client ID. Generate a client secret.
6. Add a `[forge.github]` table to `librepaper.toml`:

```toml
[forge.github]
app_slug = "your-app-slug"
client_id = "replace"
client_secret = { env = "GITHUB_APP_CLIENT_SECRET" }
```

The secret can also be a `{ file = "..." }` reference. See [Secrets](advanced.html#secrets).

7. Restart the app with `docker compose up -d`.

For GitHub Enterprise Server, register the app on that instance. Then set the two hosts. `api` is the REST API address, and `web` is the address of the GitHub site:

```toml
[forge.github]
api = "https://github.example.com/api/v3"
web = "https://github.example.com"
```
