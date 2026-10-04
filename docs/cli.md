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
librepaper admin serve --config /etc/librepaper/config.toml
librepaper admin config check --config /etc/librepaper/config.toml
librepaper admin config show --config /etc/librepaper/config.toml
librepaper admin backup --config /etc/librepaper/config.toml <dest>
librepaper admin restore --config /etc/librepaper/restore.toml <src> <dest>
curl http://127.0.0.1:8080/api/status      # check operational state
```

The TOML file is the source for server and admin storage settings. `admin
serve` defaults to `/etc/librepaper/config.toml`; other admin commands accept
the same `--config PATH`. `config show` redacts credentials and database URLs.
Restore uses a config that points at the target database and takes the new
directory as its final argument. See the [hosting page](host.html) for the
configuration format and storage requirements.

## Agents

```sh
librepaper agent list              # list configured agents
librepaper agent add <id> -- <cmd> # add agent command
librepaper agent remove <id>       # remove agent
```

See [agents page](agents.html).
