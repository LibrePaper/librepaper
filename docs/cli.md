---
title: "Command line"
---

The `librepaper` command hosts a LibrePaper server and scripts your documents. To write, install the [desktop app](desktop-app.html) instead. You do not need the command line.

## Install

The desktop packages already include the `librepaper` command: Homebrew, Scoop, the Windows installer, the `.deb`, the `.rpm`, the AUR package and Nix. See the [desktop app page](desktop-app.html) to install one of them. On a server with no desktop app, install the command alone with this installer.

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/LibrePaper/librepaper/releases/latest/download/librepaper-installer.sh | sh
```

## Host a server

```sh
librepaper admin serve --config /etc/librepaper/librepaper.toml
librepaper admin config check --config /etc/librepaper/librepaper.toml
librepaper admin config show --config /etc/librepaper/librepaper.toml
librepaper admin backup --config /etc/librepaper/librepaper.toml <dest>
librepaper admin restore --config /etc/librepaper/restore.toml <src> <dest>
curl http://127.0.0.1:8080/api/status      # check operational state
```

The application TOML file is the source for server and admin storage settings.
`admin serve` defaults to `/etc/librepaper/librepaper.toml`. Other admin
commands accept the same `--config PATH`. `config show` redacts credentials and
database URLs. Backup configuration lives separately in
`/etc/resticprofile/resticprofile.toml`. See the [hosting page](host/advanced.html#backups) for
scheduled backups and recovery.

## Script your documents

### Document commands

Pass the server address by flag or environment variable:

```sh
librepaper <COMMAND> --server https://librepaper.arelbundock.com
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"
```

### Authenticate

```sh
librepaper login      # prints code and URL to sign in (valid 90 days)
librepaper logout     # remove the cached token from this computer
```

Tokens copied elsewhere remain usable until they expire or are separately revoked. Removing the local cached token does not revoke them.

### List

```sh
librepaper list       # show ID, date, and title
```

### Export

```sh
librepaper export c9k ./paper-copy              # immutable snapshot
librepaper export c9k ./paper-copy --key URL    # read as share-link holder
librepaper export c9k ./paper-copy --at "v1"    # historical version
```
