# Komodoc

Publish an HTML or Markdown document, share its unlisted link, and collect
comments and highlights in real time.

- Highlight passages, suggest edits, and comment on figures
- Multiple people can annotate simultaneously, with live updates
- Publish and manage documents from the web or CLI
- Trivial to deploy: one static binary, on your laptop or on a small server
- Free public sandbox for small, short-lived notebooks
- Allow anonymous comments or require GitHub authentication
- Export annotations as Markdown or W3C JSON-LD

<div class="screenshot-pair">
<figure>
<img src="docs/sandbox.png" alt="Komodoc sandbox landing page with the upload area and document list">
<figcaption>The free sandbox landing page.</figcaption>
</figure>
<figure>
<img src="docs/commenting.png" alt="A document open in Komodoc with highlighted passages and the comments sidebar">
<figcaption>The annotation window, with highlights and threaded comments.</figcaption>
</figure>
</div>

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/vincentarelbundock/komodoc/main/install.sh | sh
```

The installer supports Linux and macOS. Windows binaries are available on the
[releases page](https://github.com/vincentarelbundock/komodoc/releases).

## Web interface: Try it now!

The Komodoc sandbox is a free website where anyone can upload small (<4MB) short-lived (<24hrs) HTML or Markdown files. To upload a document, you will need to log with your Github username:

[Komodoc sandbox](https://komodoc.arelbundock.com)

If you do not want to log in but want to try annotating some documents, you can try one of these live examples:

- [HTML: A Short Style Guide for Quantitative Writing](https://komodoc.arelbundock.com/docs/html-a-short-style-guide-for-quantitative-writing-72wgqautjz)
- [Markdown: What a Regression Table Is Hiding](https://komodoc.arelbundock.com/docs/markdown-what-a-regression-table-is-hiding-c9kqgt7acs)
- [Typst: What a Confidence Interval Does Not Say](https://komodoc.arelbundock.com/docs/typst-what-a-confidence-interval-does-not-say-5vvxv8ebpd)
- [Quarto: What the Bootstrap Actually Resamples](https://komodoc.arelbundock.com/docs/quarto-what-the-bootstrap-actually-resamples-j9iu5cqy7b)
- [Calepin: Newton's Method Is Not Always Your Friend](https://komodoc.arelbundock.com/docs/calepin-newton-s-method-is-not-always-your-friend-2sdp6b6aga)
- [Jupyter: Simpson's Paradox Is Not a Paradox](https://komodoc.arelbundock.com/docs/jupyter-simpson-s-paradox-is-not-a-paradox-b5serei7j7)
- [Marimo: How Far Does a Drunk Walk?](https://komodoc.arelbundock.com/docs/marimo-how-far-does-a-drunk-walk-2c9n8gwd6i)
- [Publication and management console](https://komodoc.arelbundock.com) (requires Github Login)

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. The seeded examples above
are the exception: their suffix is derived from the title rather than drawn at
random, so re-seeding the sandbox leaves these links pointing at the same
documents. That is safe only because an example is public on purpose; every
other document keeps an unguessable address. A link that resolves to nothing
gets a 404 page saying so.

<aside class="callout warning">
<strong>Warning:</strong> Do not publish confidential information on the Komodoc sandbox. Normally, documents are only visible to the person who uploaded them, or to people with the randomly generated and unlisted link. But if you are gathering comments on documents about national security, you should probably <a href="#self-managed-server">host your own instance</a> or find another solution.
</aside>

<br>

The standard web-based workflow is:

1. Open a Komodoc server in a browser, 
2. Sign in with GitHub (if the manager requires it), 
3. Upload an `.html` or `.md` file,
4. Send the (unlisted) link to your readers. 

anyone with the link can read the document. The Komodoc console only lists only the documents you own.

Click on the thumbnails near to top of this page for screenshots of the Komodoc management console and annotation page.


## CLI

Every command executed from the CLI must point to a specific Komodoc server. Typically, users will specify their server with a flag. For example, to make a request against the Komodoc sandbox, a live instance maintained by the developers, use:

```sh
komodoc <COMMAND> --server https://komodoc.arelbundock.com
```

When making repeated calls to the same server, it is convenient to specify the address using an [Environment Variable](#environment-variables). This allows us to omit the `--server` flag:

```sh
export KOMODOC_SERVER="https://komodoc.arelbundock.com"

komodoc <COMMAND>
```

In the examples below, we use the environment variables and omit the flag.

### Authenticate

Sign in once, through the deployment rather than through any one provider:

```sh
komodoc login
```

It prints an address and an eight-character code:

```text
  Open https://docs.example.org/auth/device?code=K7QD4XPM
  and enter the code:  K7QD4XPM
```

Open that page in any browser, on any machine, and sign in there with whichever
provider the deployment offers — GitHub, Google, or both. The page names the
code and the account it would sign in, and nothing happens until you press
*Approve*, so a link somebody else sends you cannot put your account on their
terminal.

The token that comes back is the deployment's own and lasts ninety days.
`komodoc logout` deletes it. It cannot be revoked one at a time: rotating the
server's session key signs every browser and every terminal out at once.

### Publish

Publish an HTML or Markdown document:

```sh
komodoc publish paper.html --title "My Paper"
```

HTML files must be self-contained, with images, styles, and fonts embedded. For
Quarto, render with:

```sh
quarto render paper.qmd --to html -M embed-resources:true
```

Publishing a file again, to a document that already exists, writes the file's
text into the live document and marks a checkpoint in its history. It never
conflicts with someone editing in the browser: their words and yours end up in
the same document, the way two browsers' do.

#### A paper is usually several files

A document is a directory, so publish the directory:

```sh
komodoc publish paper/ --title "My Paper"
```

Everything in it goes: the chapters, the `.bib`, the figures. Three things are
left behind — names beginning with a dot, the main file's own `.pdf`, and
whatever git ignores, since a `.gitignore` is the author's own statement of
what is derived. Which file is the document is the one text at the top level
that Komodoc renders, or `main.*`; when neither settles it, `--main` does:

```sh
komodoc publish paper/ --main chapters/thesis.typ
```

Publishing a single file that reads its neighbours says so rather than
publishing a document that compiles here and nowhere else — a reader renders
it themselves, and would get the error you never saw:

```
paper.typ reads lib.typ and refs.bib; publish the directory to send them along:
  komodoc publish .
```

### List

List the documents visible to your account. Each row shows a short ID (at
least three characters, and the same width for every document) along with its
date and title. The ID is a prefix of the document's suffix, so a document
you publish again gets a new one; the seeded examples below keep theirs,
because their suffix is derived rather than random:

```sh
komodoc list
```

```
2c9  2026-09-04  Marimo: How Far Does a Drunk Walk?
b5s  2026-09-04  Jupyter: Simpson's Paradox Is Not a Paradox
2sd  2026-09-04  Calepin: Newton's Method Is Not Always Your Friend
j9i  2026-09-04  Quarto: What the Bootstrap Actually Resamples
c9k  2026-09-04  Markdown: What a Regression Table Is Hiding
5vv  2026-09-04  Typst: What a Confidence Interval Does Not Say
72w  2026-09-04  HTML: A Short Style Guide for Quantitative Writing
```

### Share

A document says who may do what to it. There are four roles, as a ladder, each
including the ones beneath it:

| Role | May |
| --- | --- |
| reader | open the document and read its comments |
| commenter | comment, reply, resolve; delete their own |
| editor | edit the source; delete any comment |
| owner | share, transfer, destroy |

Everyone who has the link is a reader, and the server's `--commenters` makes
them commenters where it is open. Beyond that, a document names people:

```sh
komodoc share c9k                              # print who it is shared with
komodoc share c9k --editor annegrandchamp      # a coauthor, by GitHub account
komodoc share c9k --commenter rmcelreath
komodoc share c9k --revoke annegrandchamp
```

A grant by name is to a GitHub account, recorded by its numeric id, so it
survives a rename and follows the person across browsers. Reviewers of a paper
often have no GitHub account, and a blind reviewer must not be named at all, so
a role can also travel in a link:

```sh
komodoc share c9k --link commenter --label "reviewer 2" --until 180d
```

That prints one URL with a key in its fragment. A fragment is never sent to a
server, so the key lands in no access log and on no `Referer` header; the
document stores only its digest, which is why the key is shown once and cannot
be shown again. Links expire after six months unless `--until` says otherwise,
and `komodoc share c9k --revoke <id>` ends one early. A link names nobody, so it
can only carry a role the deployment already allows without a sign-in: on a
server that names its publishers, a link cannot edit.

Who may read is a property of the document rather than a role anyone holds:

```sh
komodoc share c9k --visibility private   # only the people named on it
komodoc share c9k --visibility listed    # anyone with the link, and on the front page
komodoc share c9k --visibility link      # anyone with the link; the default
```

A private document answers a stranger exactly as a deleted one does. Its text
is painted into the reading frame rather than served to it, because the
documents host holds no sign-in of yours to check — so a private HTML document's
own scripts do not run. Publish such a document by link instead.

The owner is one account, because the storage quota and `destroy` both need an
answer to "whose". Handing it on is its own command, confirmed the way
`destroy` is:

```sh
komodoc transfer c9k alice
```

The document, its history, its comments and its storage quota all move. An
editor cannot share: the owner is the one whose quota and whose name are on the
document. In the browser, all of this is the **Share** button in the reader,
and `komodoc list` marks the documents shared with you with the role you hold.

### Comment

Open a listed document in your browser for commenting. The ID is the one `list`
prints (a full slug also works):

```sh
komodoc comment c9k
```

### Edit

A document published from markdown or typst keeps that source, so it can be
edited in the page it is read in: the source on one side, the document as it will be
published on the other, and the comments beside both. Either of the two panes
next to the source folds away. `edit` takes the same ID `comment` does, and
just opens that page:

```sh
komodoc edit c9k
```

The editor is offered to whoever may replace the document, and the document
opens ready to work on. There is nothing to save: what is typed is the
document, readers see it a moment later, and the comments survive it — as you
type, they re-anchor against the edited text, and one whose passage is gone is
marked as needing re-anchoring rather than quietly dropped.

Several people can edit at once. The source is a CRDT (Yjs), so two people
typing in the same sentence converge without either waiting for the other, and
the toolbar says how many are in the session. The server holds the document,
relays every update and keeps the result, so closing the last tab loses nothing
and whoever opens the document next, in a browser or with `komodoc sync`, joins
what is there.

What is shared is the source. The preview is not: each browser renders what it
now has, so a session costs the deployment no CPU and no bandwidth beyond
relaying a few dozen bytes per keystroke.

History is kept for you. The server takes a checkpoint of the source when the
document has been quiet for a while, when the last editor leaves, when someone
comments, and whenever `komodoc publish` writes to it; the same text is never
checkpointed twice, and nothing is ever rewritten. Checkpoints are kept from
the first version of this, and are what a restore, a diff and the timeline in
the toolbar will be built on; none of those three exists yet.

Rendering happens in the browser, by the same compiler the command line
renders with, built for WebAssembly — for readers as much as for editors. The
deployment stores the source and nothing rendered from it, so what a reader
sees is by construction what the source says, and a live document costs the
deployment no CPU and no bandwidth beyond relaying a few dozen bytes per
keystroke.

The formats, and they are not available in the same places:

| | Published with | Renderer | Over the wire |
|---|---|---|---|
| **Markdown** | `komodoc publish paper.md` | comrak | ~130 KB compressed |
| **Typst** | `komodoc publish paper.typ` | typst | ~13 MB compressed |
| **HTML** | `komodoc publish paper.html` | the identity | nothing |
| **LaTeX** | `komodoc publish paper.tex` | a TeX the browser fetches | ~2 MB, then ~17 MB inside the first compile |

Both renderers are the same crate the binary itself renders with, compiled to
WebAssembly. Nothing else has to be installed: publishing a `.typ` file needs
no `typst` binary on your PATH, because the compiler is inside Komodoc, and it
is the same one the editor runs — so a document cannot render one way when it
is published and another way when it is edited.

The typst module is thirty megabytes, typst itself and the fonts it sets
documents in, so it is optional at build time and fetched only by someone who
opens a typst document. Both modules are fetched once and cached for a year.
Since nothing rendered is stored, a build without the typst module cannot show
a typst document at all, and refuses to create one rather than storing a source
nobody could read.

Typst's HTML export is still marked experimental upstream, so complex
documents may not survive it intact. Simple ones, including maths, come out as
real HTML with MathML, which is what lets comments anchor into them at all.

The typst renderer is built by `make typst`, which needs a Rust toolchain and
is deliberately not part of `make build`. Without it Komodoc builds and runs
exactly as before, and simply does not offer typst editing.

A document published as HTML is its own source, and its renderer is the
identity: it is shown as it was published, which it always was, and it opens in
the editor like the other two. That covers everything Quarto, Jupyter and
marimo produce, so the live preview, the co-editing and the comments reach the
documents most papers actually arrive in. A document published before HTML was
a source format needs no republishing: the HTML that is stored is its source.

The one thing to know about editing a generated file: the next `quarto render`
produces a new HTML containing none of what was typed into the old one in the
browser. The `.qmd` is where a lasting change belongs; the browser is for the
fix that cannot wait for a render.

#### LaTeX

`komodoc publish paper.tex` stores a `.tex` file as `latex`, and
`komodoc publish paper/` takes the whole directory — the chapters, the `.bib`,
the figures. Nothing is compiled on the way: Komodoc carries no TeX, no build
embeds one, and there is no `make latex`.

LaTeX is compiled in the browser instead, by a TeX distribution built for
WebAssembly that an editor's browser downloads once and keeps. The first time
you open a LaTeX document, the preview pane offers the distributions this
deployment serves and fetches nothing until you choose one; the choice is
remembered in that browser and is not asked for again, for that document or
any other. The compilers are AGPL-3.0 and MIT works of their own, fetched at
run time rather than linked into Komodoc, and the card names each licence.

Two things follow from that, and both are worth knowing before you rely on it.

Readers see nothing until an editor has opened the document. Nobody is asked
to download a compiler in order to read a paper, and storing a rendering
beside the document — so a reader gets pages rather than "not yet rendered" —
is a later step and is not built yet.

The package set is bounded, and is a mirror rather than a TeX Live. What is
carried is TeX Live's `latex-recommended`, `latex-extra`, `fonts-recommended`
and `mathscience` collections: about 190 MB of files, fetched one at a time by
name as a compile asks for them. `tikz` and `biblatex` are in collections that
are not mirrored and will not be found. And the engine's preloaded format is
LaTeX2e 2020-02-02, so a package that checks the kernel date — `siunitx` is
one — refuses to load however completely it was mirrored. When any of this
happens you get the engine's own error, in the badge and the gutter that typst
errors already appear in.

A self-hoster says where the distributions come from:

```sh
komodoc serve --latex /srv/komodoc/latex          # a mirror built by
                                                  # node latex/mirror.mjs
komodoc serve --latex https://mirror.example.com  # or a bucket serving one
komodoc serve --latex                             # or the project's own
```

Without `--latex` a deployment stores and shows `.tex` files and offers no
LaTeX editor: `/api/config` says so, and the reader offers the source rather
than the card. Whichever you pass, browsers only ever fetch `/latex/` on your
own origin — the server reads from the bucket, the browser never does, because
the list of packages a document asks for is a description of the document and
should go no further than the deployment that already has the source. An
`http:` mirror is refused at startup, since the page a document is framed in
will not load a compiler over one.

### Sync

The editor in the page is one door into a live session. `sync` is the other:
it makes the file on your disk a peer in the same session, so you can work in
vim, Positron or Emacs and still be in the document everyone else is reading.

```sh
komodoc sync c9k paper.typ
```

```
syncing paper.typ with https://komodoc.example.org/docs/coverage-t2rpf5rzq6
joined the session (2 peers)
paper.typ changed
checkpoint 4f2a91c
session changed: wrote paper.typ
```

It runs until you stop it. Edits you save flow into the session while a browser
tab is typing in it; edits made in the browser land in your file, written
atomically so your editor never reads half of one. Saving is a deliberate act,
so it asks for a checkpoint in the timeline -- a burst of saves is one mark,
and saving text the document already has is none. If the file is not there
when you start, it is written from the document, which is how you pull one
down to edit locally.

Beside a Makefile that runs `quarto render`, `komodoc sync c9k paper.html`
turns every render into a checkpoint with no step between your tools and your
readers.

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
everyone and re-anchors every comment.

### Storage

By default a server keeps everything in a directory:

```sh
komodoc serve --data ./komodoc-data
```

It can keep it in any S3-compatible bucket instead — R2, AWS, MinIO, Backblaze
— so a small server holds no durable state of its own and the bytes, the bill
and the ownership of the data are yours:

```sh
export KOMODOC_S3_ACCESS_KEY=...
export KOMODOC_S3_SECRET_KEY=...

komodoc serve --s3-endpoint https://<account>.r2.cloudflarestorage.com \
              --s3-bucket komodoc --s3-region auto
```

Credentials come from the environment by preference: a flag is visible to every
process on the machine and lands in your shell history, and komodoc says so if
you pass one.

Everything komodoc writes lives under one prefix (`komodoc/` by default,
`--s3-prefix` to change it), so a bucket can be shared and deleting a
document has a bounded blast radius. It never deletes the bucket, and never
touches a key outside its own prefix.

At startup the bucket is probed. The index is kept correct by conditional
writes, so a bucket that does not support them is refused rather than run on
quietly — pass `--single-writer` to assert that only this process writes
these keys, which is true of a single server, and it will use its own lock
instead. It is printed at startup either way.

Comments live in the bucket too (`rooms/<slug>.json`), written as they are
made. A second server pointed at the same bucket finds the room locked and
serves it read-only rather than interleaving its writes.

With `--s3-direct-reads`, a document's bytes are fetched by the reader's
browser straight from the bucket rather than passing through the server. That
needs a CORS rule on the bucket; the deployment prints the policy to paste.

### Export

Export annotations as readable Markdown. `export` takes the same ID `comment`
does, a short ID from `list`:

```sh
komodoc export c9k --format markdown --out comments.md
```

Without `--format markdown`, Komodoc exports W3C Web Annotation JSON-LD.

### Destroy

Delete one document, including its history and comments. It takes the same ID
`comment` and `export` do -- the short one from `list`, or a full slug -- and
asks you to type the full slug to confirm unless `--yes` is supplied:

```sh
komodoc destroy --document c9k
```

It deletes the document, its history and its comments. Nothing else on the
server is touched.

## Deploy

Komodoc is one static binary with everything compiled into it: the reader, the
renderers, and the server. Run it on your laptop for a quick trial, or on a
small host for something durable.

### Local

Run a public local instance with no GitHub setup at all:

```sh
komodoc serve --port 8081 --publishers YOUR-GITHUB-LOGIN
```

Open <http://localhost:8081>. Everything is stored in `komodoc-data` (see [Storage](#storage)).

### Self-managed server

Run the bundled server on your own host, with `--data` set to a persistent
directory:

```sh
komodoc serve --port 8080 --data /var/lib/komodoc --publishers YOUR-GITHUB-LOGIN
```

To let people sign in, set up a [GitHub app](#github-oauth) for this server's address.

Run the server behind a reverse proxy that terminates HTTPS, and have the proxy send the `X-Forwarded-Proto: https` header. That header is how the server knows its own address is an HTTPS one: without it the session cookie is not marked `Secure`, and uploads and comments are refused because the browser's idea of where the page came from does not match the server's. Plain HTTP is fine on `localhost` and nowhere else.

### Retention

Delete documents automatically after their most recent publication:

```sh
komodoc serve --expire-after 24h
```

For a fixed lifetime from the first upload, use `--expire-from created`. Use
`--expire-after never` to disable expiry. Expired documents are removed by an
hourly pass, and once at startup.

### Storage

`komodoc serve` keeps documents, comments and the session key in the directory
named by `--data` or `KOMODOC_DATA`, `komodoc-data` in the working directory by
default; back it up if the instance holds real work. Point it at a bucket
instead and the server holds nothing of its own — see
[Bring your own bucket](#bring-your-own-bucket).

Six flags bound what a deployment will store:

| Flag | Caps | Default |
| --- | --- | --- |
| `--max-size` | the texts of one document | 4 MB |
| `--max-assets` | the figures of one document | 32 MB |
| `--quota` | everything one publisher holds | 100 MB |
| `--storage` | the whole deployment | 5120 MB |
| `--max-documents` | documents one publisher may hold | 50 |
| `--uploads-per-hour` | uploads one publisher may make in an hour | 30 |

```sh
komodoc serve --max-size 8 --max-assets 16 --quota 500 --storage 10240
```

A document is a directory, so `--max-size` bounds the sum of its texts and
`--max-assets` bounds its figures. Both count against `--quota`; a figure is
an upload and counts against `--uploads-per-hour` like any other. On a
deployment anybody may publish to, `--max-assets` is the one worth lowering:
figures are where a paper's bytes actually are, and it is what stops a single
document spending a publisher's whole allowance on images.

Under `--publishers anyone` (see [Rights](#rights)), a browser's quota is tied
to a cookie rather than an account, so clearing cookies gets a new one;
`--storage` is the bound that actually holds under that policy. Signing in
moves what that browser published onto the account, quota and all, so the way
to stop depending on a cookie is to sign in before clearing it.

### Rights

Two flags say who may do what.
`--publishers` says who may upload documents, and `--commenters` who may
annotate them. Both accept a comma-separated list of names:

```sh
komodoc serve --publishers alice,anne@example.org --commenters @example.org
```

A name is a GitHub login, a Google account's verified email address, or a whole
domain of them; the shape of the entry is what decides which, so the forms mix
freely in one list. Besides a list, each flag takes two keywords:

| Value | Meaning |
| --- | --- |
| `alice` | the GitHub login `alice` |
| `alice@example.org` | the Google account whose verified email is that address |
| `@example.org` | any Google account on that domain |
| `any` | any signed-in account, on either provider |
| `anyone` | no sign-in at all |

A domain matches the part after the `@` exactly, so `@example.org` admits
`alice@example.org` and not `alice@mail.example.org`.

`--publishers` has no default: the server insists you say who may publish.
`anyone` is allowed, which is what makes the local trial above work without any
OAuth setup; on a host the internet can reach, name the accounts instead.
`--commenters` defaults to `anyone`,
so readers can annotate a document straight from its link; use `any` to
attribute every comment to an account, or a list to keep a draft among named
reviewers.

Both flags apply to every document alike, and both are ceilings rather than the
last word: a document may name its own coauthors and reviewers with
[Share](#share), and may only ever be stricter than the server it is on.
Nothing a document says can widen `--publishers` or `--commenters`.

`--no-listing` turns the public front page off. A document whose owner marked
it `listed` then behaves as an ordinary link, and the share dialog stops
offering the choice.

### GitHub OAuth

A server that asks anyone to sign in needs at least one OAuth client of its
own, GitHub's or Google's. A server where both `--publishers` and
`--commenters` are `anyone` never asks, and runs without either.

Create the app at [github.com/settings/developers](https://github.com/settings/developers)
(New OAuth App). Point its two URLs at the server's own address — the
public HTTPS address it sits behind, with the same `/auth/callback` path:

```text
Homepage URL:               https://docs.example.org
Authorization callback URL: https://docs.example.org/auth/callback
```

Pass the credentials to `serve` through the environment, rather than as flags: an argument is visible in `ps` to every process on the machine,
an environment variable is not.

```sh
export KOMODOC_GITHUB_CLIENT_ID="..."
export KOMODOC_GITHUB_CLIENT_SECRET="..."
```

Readers can sign in with Google instead, or as well: create a *Web application* client at [console.cloud.google.com](https://console.cloud.google.com) under *Credentials*, with the authorised redirect URI set to this server's address plus `/auth/callback/google`, and pass its id and secret as `KOMODOC_GOOGLE_CLIENT_ID` and `KOMODOC_GOOGLE_CLIENT_SECRET`. The consent screen asks for the scopes `openid`, `email` and `profile`.[^google-data] All three are non-sensitive, so the app needs no verification review — but **publish the consent screen**: one left in *Testing* admits at most a hundred named test users, and everybody else is turned away at Google's own page.

Signing in cannot be undone one account at a time. `komodoc logout` deletes a
terminal's token, and rotating the server's session key signs every browser and
every terminal out at once.

## Environment variables

[^github-data]: Komodoc requests no GitHub scopes through OAuth. It uses the
GitHub API only to obtain your public login name; it does not collect your email,
repositories, or other profile data.

[^google-data]: Komodoc reads the verified email address on a Google account,
the account identifier, and the profile name. The address is what
`--publishers`, `--commenters` and a grant by name are matched against, and
where a retention notice is sent; it is shown to no other reader anywhere.
Other readers see the profile name.

Flags take precedence over their corresponding environment variables.

| Variable | Purpose |
| --- | --- |
| `KOMODOC_SERVER` | Default server for `login`, `publish`, `list`, `export`, and document deletion |
| `KOMODOC_TOKEN` | A token to use instead of the one `komodoc login` stores; a GitHub token works too |
| `KOMODOC_DATA` | Directory `serve` and `seed` use for documents and comments (default `komodoc-data`) |
| `KOMODOC_GITHUB_CLIENT_ID` | GitHub OAuth app client ID |
| `KOMODOC_GITHUB_CLIENT_SECRET` | GitHub OAuth app client secret |
| `KOMODOC_GOOGLE_CLIENT_ID` | Google OAuth client ID, for signing in with Google |
| `KOMODOC_GOOGLE_CLIENT_SECRET` | Google OAuth client secret |
| `KOMODOC_PUBLISHERS` | Who may publish: `anyone`, `any`, or a list of logins, addresses and domains |
| `KOMODOC_COMMENTERS` | Who may comment: `anyone`, `any`, or a list of logins, addresses and domains |
| `KOMODOC_EXPIRE_AFTER` | Automatically delete documents after a duration such as `24h` or `30d` |
| `KOMODOC_EXPIRE_FROM` | Start retention at `updated` (default) or `created` |
| `KOMODOC_LATEX` | Where `serve` reads LaTeX distributions from: an https bucket or a directory |
| `KOMODOC_VERSION` | Version selected by the installer |
| `KOMODOC_BIN_DIR` | Installation directory selected by the installer |

## Building from source

Three builds go into one binary.

| | What it is | Built by |
| --- | --- | --- |
| `engine/` | markdown and typst, rendered | cargo, natively and to WebAssembly |
| `web/` | the pages: Svelte, Skeleton, CodeMirror 6, Yjs | bun and vite |
| `komodoc/` | the server and the command line | cargo |

The engine is built twice — natively into the binary, and to WebAssembly for
the browser — so the editor's preview and the command line's output come from
the same code. The web build writes into `src/shell`, which the binary embeds;
nothing under that directory is edited by hand.

```sh
make web      # the pages, from web/
make wasm     # the markdown renderer for the browser (fast)
make typst    # the typst renderer, ~30 MB (slow, and optional)
make build    # dist/komodoc, with the pages and renderers embedded
make test     # rustfmt, clippy and the test suite
```

`make build` needs [bun](https://bun.sh) and the `wasm32-unknown-unknown` target
(`rustup target add wasm32-unknown-unknown`). `make typst` is deliberately
separate: it takes a few minutes and adds thirty megabytes to the binary, and a
build without it works exactly as described above, minus typst editing.

### The look

The pages are [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton supplies
the furniture -- buttons, cards, inputs, tables, dialogs, tooltips, toasts --
and `web/src/styles/theme.css` colours all of it from Komodoc's own four
colours, so the palette is written down once and nowhere else.

Three rules keep a growing application looking like one application, and
`make test` enforces them:

1. A colour or a size comes from the theme. A hex value or an arbitrary
   Tailwind size in a component is a decision made twice.
2. A control is a component. There is one `IconButton`, so there cannot be a
   fourth kind of button that is almost like the other three.
3. Layout comes from `Page`, `Stack` and `Row`, so a new screen is assembled
   rather than measured.

Two things sit outside that system deliberately: the agent, which paints
highlights inside a document on another origin where none of this stylesheet
reaches it, and the colours identifying people in a shared editing session,
which travel over the wire to other browsers.

### The editor

The source is edited in CodeMirror 6, bound to a Yjs document. Two people
typing in the same sentence converge without either waiting for the other, each
keeps their own undo history, and each sees the other's caret where it actually
is, labelled with their name. The server relays those updates and applies them
to the copy it keeps, which is the document.

A reader who only reads fetches the page, its bundle and the Yjs document, and
renders it as it changes; CodeMirror is a separate bundle, fetched only when an
editor is actually opened.
