---
title: "The CLI"
---

LibrePaper is a server and a web app, and the command line is deliberately
small. Four commands are for a person at a terminal: `login`, `logout`,
`list`, and `export`. Two namespaces are for particular jobs: `admin` runs
a deployment and `local` runs the companion on your own computer.
Publishing, review, and document management happen in the browser.

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
directory's contents with the example documents, and `admin backup`
and `admin restore` take and restore a verified recovery point.
Operational state is one loopback request away:

```sh
curl http://127.0.0.1:8080/api/status
```

## The companion

The companion is the same binary. It runs native tools on your machine for
the jobs the browser cannot do: Quarto renders, and Typst to self-contained
HTML. It also holds the agents the document sidebar can drive.

```sh
librepaper local start       # start it in the background, or reuse the one running
librepaper local stop        # stop it
librepaper local status      # address, pairing code, pairings, tools found, agent connections
librepaper local settings    # open its settings page in a browser
librepaper local agent list  # the agents it offers; `agent add <id> -- <command>` teaches it another
```

`start --code` fixes the pairing code instead of rotating it per run, which is
what `make deploy` uses so that connecting an agent in development does not
mean reading a fresh code off a terminal after every restart. `--foreground`
runs the companion in this process instead of the background, and
`--at-login` also starts it every time you log in. `--tex-path`
(colon-separated directories) points `start` and `status` at a TeX
installation they would not otherwise find.

A Quarto or Typst document renders in a workspace of its own, written from the
files the browser sends. To render against a project folder on your disk
instead, because it keeps data the document does not share, choose the folder
under *Settings*, *Local app*, *Project folder* in the browser. The companion
never receives a path from a website; the choice is made on this machine.

Everything else is on the settings page that `librepaper local settings`
opens: build presets and their permissions, paired websites, and whether
the companion starts when you log in.

## Agents

Which agents this computer offers the document sidebar:

```sh
librepaper local agent list       # add <id> -- <command> teaches it another
```

Agents are driven from the document sidebar only; there is no command line
for connecting one to a document by hand. The [agents page](agents.html)
covers what the sidebar assistant can do.
