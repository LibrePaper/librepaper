---
title: "Command line"
---

The `librepaper` command signs in to a server, lists and exports documents, syncs folders and Git repositories, serves documents to AI agents, and administers a self-hosted server. To write, install the [desktop app](desktop-app.html) instead. You do not need the command line.

## Install

The desktop packages already include the `librepaper` command: Homebrew, Scoop, the Windows installer, the `.deb`, the `.rpm` and Nix. The plain macOS `.dmg` does not put it on your PATH. See [Install the desktop app](desktop-app.html#install-the-desktop-app). On a server with no desktop app, install the command alone with this installer:

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.ps1 | iex"
```

Check a download with `gh attestation verify <file> -R LibrePaper/librepaper`.

## Sign in

Most commands take the server address as `--server URL` or in `LIBREPAPER_SERVER`. There is no default server. A credential given with `--token` or `LIBREPAPER_TOKEN` is used instead of the stored sign-in.

```sh
export LIBREPAPER_SERVER="https://app.librepaper.org"
librepaper login      # shows a URL and a code to approve in your browser
librepaper logout     # forget the stored sign-in on this computer
```

`login` stores a token that lasts 90 days. `logout` forgets the stored sign-in for every server. Tokens copied elsewhere remain usable until they expire or are revoked. Removing the local token does not revoke them.

## Documents

```sh
librepaper list                                  # ID, date updated, and title
librepaper export c9k ./paper-copy               # the live project
librepaper export c9k ./paper-copy --at v1       # as it stood at a named version
librepaper export c9k ./paper-copy --key LINK    # as the holder of a share link
```

`list` prints a short ID for each document. Use it in place of the full ID. Documents shared with you show your role.

`export` writes a complete, independent copy of the project's source and assets into a new directory. The directory must not exist yet.

## Sync folders and Git repositories

`pull` brings a project's versions into a folder, and `push` sends the folder's changes back as one named version. The first `pull` takes the project's page address and the branch to sync. The [Git page](git.html) explains how the two commands work with a Git repository.

```sh
librepaper pull https://app.librepaper.org/docs/<slug> --branch main
librepaper push --message "Revise discussion"
librepaper push --title "Thesis draft"           # publish the folder as a new project
```

| Command | Flags |
|---|---|
| `pull` | `--dir DIR` (default `.`); `--branch BRANCH`; `--preview` shows the differences and writes nothing (first pull only); `--no-merge` fetches and stops before a Git merge; `--mine PATH` and `--theirs PATH` choose a side for a file both sides changed (outside Git) |
| `push` | `--dir DIR` (default `.`); `--message TEXT`; `--author NAME`; `--title TEXT` publishes the folder as a new project; `--branch BRANCH` |

## AI agents

`librepaper mcp` serves a project's document tools to an MCP client over stdio. Give it an agent token in `LIBREPAPER_AGENT_TOKEN`. The [AI agents](agents.html) page explains how to get a token and register the command.

## Self-hosting

The admin commands operate a server. They read a TOML file, which defaults to `/etc/librepaper/librepaper.toml`. Every admin command accepts `--config PATH`.

```sh
librepaper admin serve --config /etc/librepaper/librepaper.toml
librepaper admin migrate --config /etc/librepaper/librepaper.toml   # apply schema migrations, then exit
librepaper admin config check --config /etc/librepaper/librepaper.toml
librepaper admin config show --config /etc/librepaper/librepaper.toml
librepaper admin backup --config /etc/librepaper/librepaper.toml <dest>
librepaper admin restore --config /etc/librepaper/restore.toml <src> <dest>
curl http://127.0.0.1:8080/api/status      # operational state, on the default port
```

- `config check` validates the file without opening storage or binding ports. `config show` prints the effective settings, with credentials and database URLs redacted.
- `backup` writes a recovery point into a new directory. Give `-` in place of `<dest>` to write a tar archive to standard output.
- `restore` reads a verified backup from `<src>` and creates a new deployment in `<dest>`.
- `moderate` changes accounts and projects directly in the database, so it needs database access. Its subcommands are `block-account`, `unblock-account`, `erase-account`, `hide-project` and `unhide-project`. Each one requires `--actor` and `--reason`. `erase-account` cannot be undone.

Scheduled backups are configured in `/etc/resticprofile/resticprofile.toml`, separately from the TOML file above. See the [hosting page](host/advanced.html#backups) for scheduled backups and recovery.
