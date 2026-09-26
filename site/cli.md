---
title: "The CLI"
---

LibrePaper is a server and a web app, and the command line is deliberately
small. Four commands are for a person at a terminal: `login`, `logout`,
`list`, and `export`. Three namespaces are for particular jobs: `admin` runs
a deployment, `local` runs the companion on your own computer, and `mcp`
connects an agent to a document. Publishing, review, and document management
happen in the browser.

Every command that talks to a deployment needs to know which one. Pass it with
a flag, for example against the sandbox the developers maintain:

```sh
librepaper <COMMAND> --server https://librepaper.arelbundock.com
```

When making repeated calls to the same server, set the
[environment variable](host.html#environment-variables) once and omit the flag:

```sh
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"

librepaper <COMMAND>
```

The examples below assume the variable is set.

## Authenticate

Sign in once, through the deployment rather than through any one provider:

```sh
librepaper login
```

It prints an address and an eight-character code:

```text
  Open https://docs.example.org/auth/device?code=K7QD4XPM
  and enter the code:  K7QD4XPM
```

Open that page in any browser, on any machine, and sign in there with whichever
provider the deployment offers: GitHub, Google, or both. The page names the
code and the account it would sign in, and nothing happens until you press
*Approve*, so a link somebody else sends you cannot put your account on their
terminal.

The token that comes back is the deployment's own and lasts ninety days.
`librepaper logout` deletes it. It cannot be revoked one at a time: rotating the
server's session key signs every browser and every terminal out at once.

## List

List the documents visible to your account. Each row shows a short ID (at
least three characters, and the same width for every document) along with its
date and title. The ID is a prefix of the document's suffix. The seeded
examples keep theirs, because their suffix is derived rather than random:

```sh
librepaper list
```

```
…  2026-09-11  Learn LibrePaper with LaTeX
…  2026-09-11  Learn LibrePaper with HTML
…  2026-09-11  Learn LibrePaper with Typst
…  2026-09-11  Learn LibrePaper with Markdown
…  2026-09-11  Learn LibrePaper with Quarto
```

## Export

Export a complete independent copy of the document using the short ID from
`list` (a full slug also works):

```sh
librepaper export c9k ./paper-copy
```

The export is an immutable snapshot. Editing it does not update the hosted
document. `--key` reads as the holder of a share link rather than as your
sign-in, for a document you can open but do not own:

```sh
librepaper export c9k ./paper-copy --key https://librepaper.example/s/abc123
```

`--at` exports the project as it stood at a given label instead of its current
state. Requesting a historical export waits for the server to build its archive:

```sh
librepaper export c9k ./paper-copy --at "Draft v1"
```

## Operating a deployment

`librepaper admin serve` is the server, and its flags are the deployment's
whole configuration; `librepaper admin serve --help` lists them and the
[hosting page](host.html) explains them. `admin seed` replaces a data
directory's contents with the example documents, and `admin backup create`
and `admin backup restore` take and restore a verified recovery point.
Operational state is one loopback request away:

```sh
curl http://127.0.0.1:8080/api/status
```

## The companion

The companion is the same binary. It runs native tools on your machine for
the jobs the browser cannot do: Quarto renders, and Typst to self-contained
HTML. It also holds the agents the document sidebar can drive.

```sh
librepaper local launch           # run in the background
librepaper local start            # run in a terminal; prints a fallback pairing code
librepaper local stop             # stop the background companion
librepaper local startup enable   # optional: start when you log in
librepaper local startup disable
librepaper local status           # exits non-zero when nothing is answering
librepaper local doctor           # which tools it found, and whether it can confine them
librepaper local manage           # open the companion's settings page
librepaper local disconnect --all # revoke every paired site
```

`start --code` fixes the pairing code instead of rotating it per run, which is
what `make deploy` uses so that connecting an agent in development does not
mean reading a fresh code off a terminal after every restart. `--tex-path`
(colon-separated directories) points `start` and `doctor` at a TeX
installation they would not otherwise find.

A Quarto or Typst document renders in a workspace of its own, written from the
files the browser sends. To render against a project folder on your disk
instead, because it keeps data the document does not share, choose the folder
under *Settings*, *Local app*, *Project folder* in the browser. The companion
never receives a path from a website; the choice is made on this machine.

Build presets are how the companion offers a configured adapter, wrapper, or
environment to the browser's build settings without any of it crossing the
wire:

```sh
librepaper local preset list
librepaper local preset create NAME ADAPTER --format typst --option engine=lualatex
```

## Agents

Which agents this computer offers the document sidebar, and what they can
reach:

```sh
librepaper local agent list       # add <id> -- <command> teaches it another
librepaper local connections      # which documents agents here can reach
librepaper local connections --remove NAME
```

An agent's own configuration points at a document through a named connection,
so the document key never sits in a config file or a shell history:

```sh
librepaper mcp --connection thesis
```

The [agents page](agents.html) covers how connections are made and what the
tools can do.
