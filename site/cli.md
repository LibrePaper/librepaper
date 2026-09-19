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

When making repeated calls to the same server, it is convenient to specify the address using an [environment variable](host.html#environment-variables). This allows us to omit the `--server` flag:

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

## Publish

Publish an HTML, Markdown, or Quarto document:

```sh
librepaper publish paper.html --title "My Paper"
```

An HTML file is accepted as source and may be self-contained, with images,
styles, and fonts embedded. For a Quarto project, publish `paper.qmd` and its
declared input files instead of uploading a generated HTML result. This
preserves the source and does not run its code. See [Quarto documents](authoring/quarto.html)
for browser preview and local rendering.

Publishing a file again, to a document that already exists, writes the file's
text into the live document and marks a checkpoint in its history. It never
conflicts with someone editing in the browser: their words and yours end up in
the same document, the way two browsers' do.

### Several files

A document is a directory, so publish the directory:

```sh
librepaper publish paper/ --title "My Paper"
```

Everything in it goes: the chapters, the `.bib`, the figures. Three things are
left behind: names beginning with a dot, the main file's own `.pdf`, and
whatever git ignores, since a `.gitignore` is the author's own statement of
what is derived. Which file is the document is the one text at the top level
that LibrePaper renders, or `main.*`; when neither settles it, `--main` does:

```sh
librepaper publish paper/ --main chapters/thesis.typ
```

Quarto projects use the narrower [sharing policy](authoring/quarto.html),
which also excludes raw data, execution caches, and generated output by default.

Publishing a single file that reads its neighbours says so rather than
publishing a project that compiles here and nowhere else. An editor opening
the project would otherwise be unable to reproduce its rendering:

```
paper.typ reads lib.typ and refs.bib; publish the directory to send them along:
  librepaper publish .
```

## Sync

The editor in the page is one door into a live session. `sync` is the other:
it makes the file on your disk a peer in the same session, so you can work in
vim, Positron or Emacs and still be in the document everyone else is reading.

```sh
librepaper sync c9k paper.typ
```

```
syncing paper.typ with https://librepaper.example.org/docs/coverage-t2rpf5rzq6
joined the session (2 peers)
paper.typ changed
checkpoint 4f2a91c
session changed: wrote paper.typ
```

It runs until you stop it. Edits you save flow into the session while a browser
tab is typing in it; edits made in the browser land in your file, written
atomically so your editor never reads half of one. Saving is a deliberate act,
so it asks for a checkpoint in the timeline -- and saving text the document
already has is none, because a checkpoint whose tree is the newest
checkpoint's tree writes nothing. If the file is not there
when you start, it is written from the document, which is how you pull one
down to edit locally.

For Quarto source documents, synchronize `paper.qmd` and its declared inputs.
Synchronizing `paper.html` is supported only when that HTML is deliberately
published as the document's source; generated preview output is not retained.

**The one thing worth understanding.** Your editor is a snapshot client: it
read the file at some moment and writes its whole buffer back when you save.
If the session moved in between -- you in a browser tab, a coauthor, a restore
-- your buffer knows nothing about it. So a save is merged rather than
applied: edits to different paragraphs all go through, and where both sides
changed the same words the session wins, your file is brought forward, and the
terminal says which words it gave up. The merge is by word, not by line,
because a paragraph here is one line.

It is not live collaboration from vim: your side of the session moves when the
file is written. Turn on your editor's auto-save if you want it to move often.
Two people typing in the same paragraph at the same time is what the browser
editor is for.

One file, one document, one session. `--interval` (default `250ms`) sets how
long either side stays quiet before it is acted on. A lock beside the file
stops you running two of these on it by accident. Turn off format-on-save for
a synced file: a formatter that rewrites every line is a change against
everyone and makes the server recompute where each source-anchored comment belongs.

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

## Export

Export a document's comments, as W3C Web Annotation JSON-LD by default or as
Markdown:

```sh
librepaper export c9k --format markdown --output comments.md
```

`--format response` writes a reply template listing each comment with room
under it, and `--since <checkpoint>` limits that to comments made after a given
moment:

```sh
librepaper export c9k --format response --since 4f2a91c --output response.md
```

`--project` takes a complete independent copy of the document instead:

```sh
librepaper export c9k --project --output ./paper-copy
```

An exported project is a snapshot. Editing it does not update the hosted
document.

## The companion

The companion is the same binary. It runs native tools on your machine for the
jobs the browser cannot do: Quarto renders, and Typst to self-contained HTML.

```sh
librepaper local launch           # run in the background
librepaper local start            # run in a terminal; prints a fallback pairing code
librepaper local stop             # stop the background companion
librepaper local startup enable   # optional: start when you log in
librepaper local startup disable
librepaper local status           # exits non-zero when nothing is answering
librepaper local doctor           # which tools it found, and whether it can confine them
librepaper local disconnect --all
```

`start --code` fixes the pairing code instead of rotating it per run, which is
what `make deploy` uses so that connecting an agent in development does not
mean reading a fresh code off a terminal after every restart.

```sh
librepaper local start --code 123456
```

Which agents this computer offers the document sidebar, and what they can
reach:

```sh
librepaper local agent list       # add <id> -- <command> teaches it another
librepaper local connections      # which documents agents here can reach
librepaper local connections --remove NAME
```

`local start` takes `--code` to fix the pairing code instead of a fresh random
one each run, and `--tex-path` (colon-separated directories) when a TeX
installation lives somewhere `start` and `doctor` would not otherwise search.

A Quarto document can render against a project folder on your disk rather than
the hosted workspace:

```sh
librepaper local quarto bind https://your-librepaper-server.example <document-slug> \
  --root /path/to/project --main paper.qmd
```

Revoke it with `librepaper local quarto unbind <binding>`. For parser
inspection without execution, use `librepaper quarto inspect paper.qmd`.
