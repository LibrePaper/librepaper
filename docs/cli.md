---
title: "The CLI"
---

## Desktop app

The [desktop app](desktop-app.html) runs its own local service. You do not start, stop or pair it from the CLI.

The desktop app finds document tools from the `PATH` it inherits when it starts. Set a tool's executable explicitly in its Integrations settings when it is not on `PATH`.

For local backups, see [backups](backups.html).

## Export

```sh
librepaper export c9k ./paper-copy              # immutable snapshot
librepaper export c9k ./paper-copy --key URL    # read as share-link holder
librepaper export c9k ./paper-copy --at "v1"    # historical version
```

## Document commands

Pass the server address by flag or environment variable:

```sh
librepaper <COMMAND> --server https://librepaper.arelbundock.com
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"
```

## Authenticate

```sh
librepaper login      # prints code and URL to sign in (valid 90 days)
librepaper logout     # remove the cached token from this computer
```

Tokens copied elsewhere remain usable until they expire or are separately revoked. Removing the local cached token does not revoke them.

## List

```sh
librepaper list       # show ID, date, and title
```

## Self-host

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

## Agents

```sh
librepaper agent list              # list configured agents
librepaper agent add <id> -- <cmd> # add agent command
librepaper agent remove <id>       # remove agent
```

See [agents page](agents.html).
