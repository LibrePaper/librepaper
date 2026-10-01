---
title: "The CLI"
---

LibrePaper is a server and a web app. User commands are `login`, `logout`, `list`, and `export`. Two namespaces handle specialized jobs: `admin` for deployment, and `local` for the companion on your machine.

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

To keep scheduled local ZIP copies of every project owned by the signed-in
account, use [account backups](backups.html) in the browser settings. This uses
the companion and its native destination picker rather than a one-off CLI
export.

## Operating a deployment

- `librepaper admin serve` starts the server. See `--help` and the [hosting page](host.html) for flags.
- `librepaper admin backup` and `librepaper admin restore` create and restore verified recovery points.
- Check operational state: `curl http://127.0.0.1:8080/api/status`

## The companion

The companion runs native tools (Quarto and Typst rendering) and holds document agents.

```sh
librepaper local start                          # start in background
librepaper local stop                           # stop it
librepaper local status                         # address, pairings, tools, agents
librepaper local approve <code>                 # approve on no-display machine
librepaper local disconnect <origin>            # revoke a website's pairing
librepaper local agent list                     # list agents; add <id> -- <cmd> adds one
```

Use `--foreground` to run in this process, `--at-login` to auto-start when you log in, or `--tool-path` (colon-separated dirs) to find tools.

To render a document against a project folder on your disk, choose the folder under *Settings*, *Local app*, *Project folder* in the browser. Permissions and presets are also configured there. Any permission request triggers a dialog on this machine, which no website can click. On a machine with no display, check `companion.log` for the code and run `librepaper local approve <code>` within 5 minutes.

## Agents

```sh
librepaper local agent list       # driven from document sidebar only
librepaper local agent add <id> -- <command>
```

See [agents page](agents.html).
