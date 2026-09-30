---
title: "The CLI"
---

LibrePaper is a server and a web app. User commands: `login`, `logout`, `list`, `export`. Namespaces: `admin` (deployment), `local` (companion on your machine).

```sh
# Pass --server or set LIBREPAPER_SERVER env var
librepaper <COMMAND> --server https://librepaper.arelbundock.com
# Or once:
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

## Operating a deployment

- `librepaper admin serve` - the server; see `--help` and [hosting page](host.html) for flags
- `librepaper admin backup` / `admin restore` - recovery points
- Operational state: `curl http://127.0.0.1:8080/api/status`

## The companion

Runs native tools (Quarto, Typst) and holds document agents.

```sh
librepaper local start                          # start in background
librepaper local stop                           # stop it
librepaper local status                         # address, pairings, tools, agents
librepaper local approve <code>                 # approve on no-display machine
librepaper local disconnect <origin>            # revoke a website's pairing
librepaper local agent list                     # list agents; add <id> -- <cmd> adds one
```

Flags: `--foreground` (run in this process), `--at-login` (auto-start), `--tool-path` (colon-separated dirs).

Project folder render: choose under *Settings*, *Local app*, *Project folder* in browser. Permissions and presets also there. Dialog approvals on this machine; no-display machines: check `companion.log` for code, approve within 5 minutes.

## Agents

```sh
librepaper local agent list       # driven from document sidebar only
librepaper local agent add <id> -- <command>
```

See [agents page](agents.html).
