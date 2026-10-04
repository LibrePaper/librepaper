---
title: "The CLI"
---

## The companion

After [installing LibrePaper](install.html), run `librepaper` in a terminal to start it in the background.

```sh
librepaper                                      # start in background
librepaper start                                # explicit equivalent
librepaper install-desktop                      # install a launcher that starts the companion
librepaper --at-login                           # start now and at login
librepaper --foreground                        # run in this process
librepaper --port 8763                          # use a chosen port
librepaper status                               # address, pairings, tools, agents
librepaper stop                                 # stop it
librepaper agent list                           # list configured agents
librepaper agent add <id> -- <command>          # add an agent command
librepaper agent remove <id>                    # remove an agent
librepaper local approve <code>                 # approve on a no-display machine
librepaper local disconnect <origin>            # revoke a website pairing
librepaper export c9k ./paper-copy              # immutable snapshot
librepaper export c9k ./paper-copy --key URL    # read as share-link holder
librepaper export c9k ./paper-copy --at "v1"    # historical version
```

On supported desktops, starting the companion shows a tray icon. Choose **Settings** from its menu to open the Companion section in the main LibrePaper app. The tray appears automatically; there is no tray preference to configure. Closing the app page leaves the companion running; use **Quit companion** in Settings or `librepaper stop` to stop it. Start-at-login is optional. Folder selection uses the operating system's native chooser.

The companion discovers document tools from the `PATH` inherited when it starts. Set a tool's executable explicitly in its Integrations settings when it is not on `PATH`.

On a machine with no display, check `companion.log` for the approval code and run `librepaper local approve <code>` within two minutes. The CLI remains available for headless setup and control.

For scheduled local backups, see [account backups](backups.html).

## Document commands

Pass server address with flag or environment variable:

```sh
librepaper <COMMAND> --server https://librepaper.arelbundock.com
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"
```

## Authenticate

```sh
librepaper login      # prints code and URL to sign in (valid 90 days)
librepaper logout     # remove the cached token from this computer
```

Tokens copied elsewhere remain usable until they expire or are separately revoked; removing the local cached token does not revoke them.

## List

```sh
librepaper list       # show ID, date, and title
```

## Self-host

```sh
librepaper admin serve --help              # start server with flags
librepaper admin backup <dest>             # backup default data directory
librepaper admin backup --data-directory <data-dir> <dest>
librepaper admin restore <src> <dest>      # restore from backup
curl http://127.0.0.1:8080/api/status      # check operational state
```

See [hosting page](host.html) for server flags.

The backup command uses the default data directory `librepaper-data`, or the
`LIBREPAPER_DATA` environment variable, unless `--data-directory` is supplied.
Self-hosted backups also require a PostgreSQL database and restore into an
empty database and a new directory; see [storage requirements](host.html#storage).

## Agents

```sh
librepaper agent list              # list configured agents
librepaper agent add <id> -- <cmd> # add agent command
librepaper agent remove <id>       # remove agent
```

See [agents page](agents.html).
