---
title: "The CLI"
---

## The companion

After [installing LibrePaper](install.html), run `librepaper` in a terminal to start it in the background.

```sh
librepaper                                      # start in background
librepaper start                                # explicit equivalent
librepaper --at-login                           # start now and at login
librepaper --foreground                        # run in this process
librepaper --port 8763                          # use a chosen port
librepaper --tool-path /opt/tools:/usr/local/bin # extra tool search paths
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

On a machine with no display, check `companion.log` for the code and run `librepaper local approve <code>` within 5 minutes.

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
librepaper logout     # revoke token
```

## List

```sh
librepaper list       # show ID, date, and title
```

## Self-host

```sh
librepaper admin serve --help              # start server with flags
librepaper admin backup <dir> <dest>       # create recovery point
librepaper admin restore <src> <dest>      # restore from backup
curl http://127.0.0.1:8080/api/status      # check operational state
```

See [hosting page](host.html) for server flags.

## Agents

```sh
librepaper agent list              # list configured agents
librepaper agent add <id> -- <cmd> # add agent command
librepaper agent remove <id>       # remove agent
```

See [agents page](agents.html).
