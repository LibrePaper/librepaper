# Komodoc

Publish an HTML or Markdown document, share a link to it, and collect
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
<img src="docs/images/sandbox.png" alt="Komodoc sandbox landing page with the upload area and document list">
<figcaption>The free sandbox landing page.</figcaption>
</figure>
<figure>
<img src="docs/images/commenting.png" alt="A document open in Komodoc with highlighted passages and the comments sidebar">
<figcaption>The annotation window, with highlights and threaded comments.</figcaption>
</figure>
</div>

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/vincentarelbundock/komodoc/main/deploy/install.sh | sh
```

The installer supports Linux and macOS. Windows binaries are available on the
[releases page](https://github.com/vincentarelbundock/komodoc/releases).

## Web interface: Try it now!

The Komodoc sandbox is a free website where anyone can upload small (<4MB) short-lived (<24hrs) HTML or Markdown files. To upload a document, you will need to log with your Github username:

[Komodoc sandbox](https://komodoc.arelbundock.com)

If you do not want to log in but want to try annotating some documents, you can try one of these live examples:

- [Markdown: What a Regression Table Is Hiding](https://komodoc.arelbundock.com/docs/markdown-what-a-regression-table-is-hiding-c9kqgt7acs)
- [Typst: What a Confidence Interval Does Not Say](https://komodoc.arelbundock.com/docs/typst-what-a-confidence-interval-does-not-say-5vvxv8ebpd)
- [HTML: What the Bootstrap Actually Resamples](https://komodoc.arelbundock.com/docs/html-what-the-bootstrap-actually-resamples-g6zm9dbzpa) (rendered by Quarto)
- [LaTeX: What a Standard Error Assumes](https://komodoc.arelbundock.com/docs/latex-what-a-standard-error-assumes-75x2fwzpc8)
- [Publication and management console](https://komodoc.arelbundock.com) (requires Github Login)

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. The seeded examples above
are the exception: their suffix is derived from the title rather than drawn at
random, so re-seeding the sandbox leaves these links pointing at the same
documents. That is safe only because an example is public on purpose; every
other document keeps an unguessable address. A link that resolves to nothing
gets a 404 page saying so.

<aside class="callout warning">
<strong>Warning:</strong> Do not publish confidential information on the Komodoc sandbox. Normally, documents are only visible to the person who uploaded them, or to people holding a share link they minted. But if you are gathering comments on documents about national security, you should probably <a href="#self-managed-server">host your own instance</a> or find another solution.
</aside>

<br>

The standard web-based workflow is:

1. Open a Komodoc server in a browser, 
2. Sign in with GitHub (if the manager requires it), 
3. Upload an `.html` or `.md` file,
4. Send the read link to your readers, or mint a comment link and send that.

Only somebody holding a live link can open the document; its bare URL opens
for you alone. The Komodoc console lists only the documents you own or have
been let into.

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
provider the deployment offers â GitHub, Google, or both. The page names the
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
left behind â names beginning with a dot, the main file's own `.pdf`, and
whatever git ignores, since a `.gitignore` is the author's own statement of
what is derived. Which file is the document is the one text at the top level
that Komodoc renders, or `main.*`; when neither settles it, `--main` does:

```sh
komodoc publish paper/ --main chapters/thesis.typ
```

Publishing a single file that reads its neighbours says so rather than
publishing a document that compiles here and nowhere else â a reader renders
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
75x  2026-09-06  LaTeX: What a Standard Error Assumes
g6z  2026-09-06  HTML: What the Bootstrap Actually Resamples
5vv  2026-09-06  Typst: What a Confidence Interval Does Not Say
c9k  2026-09-06  Markdown: What a Regression Table Is Hiding
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

A document is shared with links, not people, and a link is the only way in.
There are four: the owner's own, which is the document's bare URL and opens
for the owner's sign-in alone, and three the owner mints, one per role --
read, comment, edit -- each of which can be created, replaced, revoked and
given an expiry. Minting one is the whole act of sharing:

```sh
komodoc share c9k                       # print the owner's link and every role's link
komodoc share c9k --link comment        # mint (or rotate) the comment link
komodoc share c9k --link edit --until 30d
komodoc share c9k --link comment --label "Review bot" --budget 20
komodoc share c9k --revoke edit         # turn the edit link off
```

That prints one URL with a key in its fragment. A fragment is never sent to a
server, so the key lands in no access log and on no `Referer` header. Minting a
role's link again rotates it: the old key dies and the new one takes over,
which is how a leaked link is killed without losing the role it stood for.
Links expire after six months unless `--until` says otherwise (`--until never`
for one that does not). `komodoc publish` mints the read link when it creates
a document and prints that, with no expiry, so what it prints is the thing to
send; revoke it and the document is yours alone until you mint another.

A link may have a label, which is only the owner's memo about what the one
role link is for, and a comments-per-hour budget. Every person or machine
holding that link shares its budget, even from different addresses; without
`--budget`, the deployment's ordinary comment limit applies. The label
survives a rotation unless another is supplied, and both values disappear
when the link is revoked. The same controls are beside each role in the
browser's **Share** pane.

A read link is read-only, whatever `--commenters` says: the switch is a
ceiling on what a link may carry, not a grant to whoever reaches the document.
An edit link authorizes, and the account attributes: editing requires an
account wherever `--publishers` does, so on a server that names its publishers
the holder of an edit link must sign in as one of them before the link edits,
and until then it only comments. Under `--publishers anyone` it edits as it
is. Anonymous commenters still need telling apart, so each gets a stable
per-document pseudonym such as `AmberAgama`, shown next to their comments
instead of a name they typed.

A stranger -- anyone with the URL and no live link -- is answered exactly as
a deleted document answers. The reading frame is served from a separate
documents host that holds no sign-in of yours, so the reader fetches a
short-lived token on the origin that does and puts it on the frame's URL;
that is what lets an HTML document's own scripts run for whoever may read it
and for nobody else.

Named grants -- an editor or a commenter added by GitHub login, from before
links existed -- are legacy: still honoured, still revocable by that login,
but a document never grows new ones. `komodoc share c9k` lists any that remain
under a `people (legacy)` heading.

Every command that acts on one document takes `--key` with a link, or the key
out of one, and then acts as that link's holder rather than as your sign-in:

```sh
komodoc comment c9k --key 'https://komodoc.example.org/docs/c9k#k=…'
komodoc sync c9k paper.typ --key …      # an edit link; no login needed where
                                        # the deployment asks for none
komodoc export c9k --key …
```

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

Open a document in your browser for commenting. The ID is the one `list`
prints (a full slug also works), and `--key` takes a link you were sent:

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

The **Files** sidebar is a folder tree. Its toolbar creates files and folders
inside the selection, or uploads files from your computer. Drag files or
folders onto another folder to move them; drop onto empty space in the sidebar
to move them to the top level. Dropping files or directories from your computer uploads them with
their folder structure. Upload name collisions offer **Keep both** or **Skip**.

Right-click an item or use its **⋯** menu to rename, move, download, or delete
it; files can also be duplicated. Use Ctrl/Cmd-click or Shift-click to select
several items, F2 to rename, and Delete to open the deletion confirmation.
Empty folders survive browser reloads and are included in browser ZIP
downloads. Moves preserve collaborative text editing, but references in source
files are not rewritten. A folder containing the main file cannot be deleted
until another file is made the main file.

The editor is offered to whoever may replace the document, and the document
opens ready to work on. There is nothing to save: what is typed is the
document, readers see it a moment later, and the comments survive it â as you
type, they re-anchor against the edited text. A comment records its passage in
the source file as well as on the page, so it can be found in any version and
in the editor, and one whose passage is gone from both is marked as such
rather than quietly dropped.

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
comments, and whenever `komodoc publish` writes to it. Unchanged text reuses its
checkpoint; an explicit restore records a new event. The history panel lets you
read earlier versions, compare changes, and restore a whole version or bring
back individual passages in the editor.

Rendering happens on clients. Markdown readers render HTML in the browser;
Typst editors compile PDFs in a WebAssembly worker, using the same compiler
as the command line. The deployment stores Typst and LaTeX PDFs under the
digest of their source tree, so readers can view them without downloading a
compiler. The server synchronizes source and stores artifacts; it does not
compile documents.

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
is the same one the editor runs â so a document cannot render one way when it
is published and another way when it is edited.

The Typst module contains the compiler and embedded fonts. It is optional at
build time and downloaded automatically when an editor needs it, without a
TeX distribution chooser. Renderer URLs include their content digest and are
cached for a year. A deployment without Typst WASM can still serve stored PDFs.

Typst uses its paged PDF exporter, preserving page layout, columns, headers,
footers, and typography. The shared PDF viewer supplies selectable text for
comments and highlights. Source navigation matches visible text; generated
text and formulas can have no match. Existing project-file and package
resolution limits still apply.

The first successful PDF is stored immediately; later versions are stored
after the source stays quiet or a checkpoint is named. A compile error keeps
the last successful preview and shows diagnostics. Documents created through
the source API, and older Typst documents without a stored PDF, show "Not yet
rendered" until an editor compiles them. Native Typst publishing uploads its
PDF when the compiled inputs match the published project.

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
`komodoc publish paper/` takes the whole directory â the chapters, the `.bib`,
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
beside the document â so a reader gets pages rather than "not yet rendered" â
is a later step and is not built yet.

The package set is bounded, and is a mirror rather than a TeX Live. What is
carried is TeX Live's `latex-recommended`, `latex-extra`, `fonts-recommended`
and `mathscience` collections: about 190 MB of files, fetched one at a time by
name as a compile asks for them. `tikz` and `biblatex` are in collections that
are not mirrored and will not be found. And the engine's preloaded format is
LaTeX2e 2020-02-02, so a package that checks the kernel date â `siunitx` is
one â refuses to load however completely it was mirrored. When any of this
happens you get the engine's own error, in the badge and the gutter that typst
errors already appear in.

A self-hoster says where the distributions come from:

```sh
komodoc serve --latex /srv/komodoc/latex          # a mirror built by
                                                  # node latex/tools/mirror.mjs
komodoc serve --latex https://mirror.example.com  # or a bucket serving one
komodoc serve --latex                             # or the project's own
```

Without `--latex` a deployment stores and shows `.tex` files and offers no
LaTeX editor: `/api/config` says so, and the reader offers the source rather
than the card. Whichever you pass, browsers only ever fetch `/latex/` on your
own origin â the server reads from the bucket, the browser never does, because
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

It can keep it in any S3-compatible bucket instead â R2, AWS, MinIO, Backblaze
â so a small server holds no durable state of its own and the bytes, the bill
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
quietly â pass `--single-writer` to assert that only this process writes
these keys, which is true of a single server, and it will use its own lock
instead. It is printed at startup either way.

Comments live in the bucket too (`rooms/<slug>.json`), written as they are
made. A second server pointed at the same bucket finds the room locked and
serves it read-only rather than interleaving its writes.

### History

A document is never lost, and its past is never rewritten. The server takes a
checkpoint when the typing stops, when the last editor leaves, whenever
somebody comments, and whenever the document is published to; each one records
the whole directory at that moment, so a chapter and the file that includes it
can never come back out of step.

```sh
komodoc history c9k
```

```
sha      at                    by                  why      label
8b03d77  2026-09-03 09:12:40   vincentarelbundock  cli
4f2a91c  2026-09-05 14:02:11   vincentarelbundock  cli      sent to the journal
c07e1aa  2026-09-05 16:40:03   annegrandchamp      comment
d1e0f42  2026-09-05 17:02:19   vincentarelbundock  left     *
```

Name a moment so it stands out, and so it is the last thing shed if a quota
ever bites:

```sh
komodoc label c9k 4f2a91c "sent to the journal"
komodoc label c9k 4f2a91c            # and to take the name off again
```

In the reader, the history button opens the same list beside the document.
Picking a moment shows the document as it was at that moment, with a bar
saying which one; "Back to now" returns to the live document. Copying a
checkpoint's link preserves the share key that gave you access. Editors can
name checkpoints and restore earlier versions.

"What changed since" compares an earlier checkpoint with the current visible
text. Click an inserted or replaced passage to find it in the document;
deleted passages retain surrounding words in the list. Changed file paths
open source comparisons, and editors can compare two checkpoints and bring
individual changes into the live source.

The same comparisons and whole-version restore are available in the terminal:

```sh
komodoc diff c9k 8b03d77 4f2a91c
komodoc restore c9k 8b03d77
```

Restore requires editor access. It records the current version before applying
the earlier directory through the shared editing session, then records the
restore in history. Both versions remain available, subject to the deployment's
history quota. Use `--key` with a share link on either command.

### Export

Export annotations as readable Markdown. `export` takes the same ID `comment`
does, a short ID from `list`:

```sh
komodoc export c9k --format markdown --out comments.md
```

Without `--format markdown`, Komodoc exports W3C Web Annotation JSON-LD.

### Response to reviewers

The one export that is not a list of what was said. `--format response` writes
the document an author has to produce anyway: grouped by reviewer, numbered
within each, with the remark, the passage as that reviewer saw it, what became
of it since, and the thread underneath as the answer.

```sh
komodoc export c9k --format response --since 4f2a91c --out response.md
```

```markdown
## Reviewer: annegrandchamp

### 1. commenting, resolved in d1e0f42

> The confidence interval does not say that the parameter is inside it with 95% probability.

**Then:** “with 95% probability, the true value lies in the interval”

**Now:** no longer in the document.

**Vincent:** Fixed as suggested; see also the new footnote on coverage.
```

Replying to a comment in the reader is writing this document. `--since` takes
a checkpoint from `komodoc history` and keeps the comments made at or after
it, which is a round of review.

**Then** is a quotation rather than a recollection, because every comment
records the checkpoint it was made on. **Now** says whether the passage is
still in the document and quotes its replacement when the word diff can
identify it. The line is left out entirely for a
document this machine cannot render -- a LaTeX paper, whose compiler is in a
browser.

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
instead and the server holds nothing of its own â see
[Bring your own bucket](#bring-your-own-bucket).

Six flags bound what a deployment will store:

| Flag | Caps | Default |
| --- | --- | --- |
| `--max-size` | the texts of one document, and any one rendering of it | 4 MB |
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
an upload and counts against `--uploads-per-hour` like any other. A Typst or
LaTeX document also keeps the PDF a client compiled, so that a reader
never has to compile one: `--max-size` bounds that PDF too, it counts against
the quota and the hourly uploads like a figure, and only the newest one plus
the named checkpoints' are kept. On a
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

`--no-listing` turns the public front page off: the reserved examples stop
being listed to people who hold nothing on them, and nothing else was ever
listed to strangers.

### GitHub OAuth

A server that asks anyone to sign in needs at least one OAuth client of its
own, GitHub's or Google's. A server where both `--publishers` and
`--commenters` are `anyone` never asks, and runs without either.

Create the app at [github.com/settings/developers](https://github.com/settings/developers)
(New OAuth App). Point its two URLs at the server's own address â the
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

Readers can sign in with Google instead, or as well: create a *Web application* client at [console.cloud.google.com](https://console.cloud.google.com) under *Credentials*, with the authorised redirect URI set to this server's address plus `/auth/callback/google`, and pass its id and secret as `KOMODOC_GOOGLE_CLIENT_ID` and `KOMODOC_GOOGLE_CLIENT_SECRET`. The consent screen asks for the scopes `openid`, `email` and `profile`.[^google-data] All three are non-sensitive, so the app needs no verification review â but **publish the consent screen**: one left in *Testing* admits at most a hundred named test users, and everybody else is turned away at Google's own page.

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
| `crates/engine/` | markdown and typst, rendered | cargo, natively and to WebAssembly |
| `web/` | the pages: Svelte, Skeleton, CodeMirror 6, Yjs | bun and vite |
| `crates/komodoc/` | the server and the command line | cargo |

The engine is built twice â natively into the binary, and to WebAssembly for
the browser â so the editor's preview and the command line's output come from
the same code. The web build writes into `web/dist`, which the binary embeds;
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
