---
title: "The CLI"
---

The CLI is the bridge between a local project and LibrePaper. Its everyday
commands are `login`, `logout`, `publish`, `open`, `sync`, `list`, and
`export`; review and document management happen in the browser. Every command
executed from the CLI must point to a specific LibrePaper server. Typically,
users will specify their server with a flag. For example, to make a request
against the LibrePaper sandbox, a live instance maintained by the developers,
use:

```sh
librepaper <COMMAND> --server https://librepaper.arelbundock.com
```

When making repeated calls to the same server, it is convenient to specify the address using an [environment variable](../host/storage.md). This allows us to omit the `--server` flag:

```sh
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"

librepaper <COMMAND>
```

Operator commands live under `librepaper admin` (including `serve`, `status`,
backups, seeding, and link-key rotation). `local`, `quarto`, `agent`, and
`skills` remain specialist namespaces for integrations and local tooling.

In the examples below, we use the environment variables and omit the flag.

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
date and title. The ID is a prefix of the document's suffix, so a document
you publish again gets a new one; the seeded examples below keep theirs,
because their suffix is derived rather than random:

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

## Open

Open a document in the browser using the short ID from `list` (a full slug also
works):

```sh
librepaper open c9k
```
