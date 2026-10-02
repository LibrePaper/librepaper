---
title: "The CLI"
---

LibrePaper is a server and a web app. The installed command starts its local companion; user commands also include `login`, `logout`, `list`, and `export`. `admin` operates a deployment.

## The companion

The companion runs native tools (Quarto and Typst rendering) and holds document agents. After installing LibrePaper, run `librepaper` in a terminal to start it in the background. The explicit `start` command does the same thing.

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
```

The older `librepaper local start`, `stop`, `status`, and `agent` commands remain available for compatibility. Prefer the root commands shown above.

## Document commands

Pass the server address with a flag or set an environment variable once:

```sh
librepaper <COMMAND> --server https://librepaper.arelbundock.com
# Or set once and omit the flag:
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"
librepaper <COMMAND>
```

## Authenticate

```sh
librepaper login      # prints a code and URL
librepaper logout     # deletes token (valid 90 days)
```

Open the URL in any browser to sign in. Press *Approve* to add the token to your terminal.

## List

```sh
librepaper list       # shows ID, date, and title for each document
```

## Export

```sh
librepaper export c9k ./paper-copy              # immutable snapshot
librepaper export c9k ./paper-copy --key URL    # read as share-link holder
librepaper export c9k ./paper-copy --at "v1"    # historical snapshot
```

To keep scheduled local ZIP copies of every project available to the signed-in
account, including shared projects, use [account backups](backups.html) in the
browser settings. This uses the companion and its native destination picker
rather than a one-off CLI export.

## Operating a deployment

- `librepaper admin serve` starts the server. See `--help` and the [hosting page](host.html) for flags.
- `librepaper admin backup` and `librepaper admin restore` create and restore verified recovery points.
- Check operational state: `curl http://127.0.0.1:8080/api/status`

## Agents

```sh
librepaper agent list       # driven from document sidebar only
librepaper agent add <id> -- <command>
```

See [agents page](agents.html).
