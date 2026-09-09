# LibrePaper

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
<img src="docs/images/sandbox.png" alt="LibrePaper sandbox landing page with the upload area and document list">
<figcaption>The free sandbox landing page.</figcaption>
</figure>
<figure>
<img src="docs/images/commenting.png" alt="A document open in LibrePaper with highlighted passages and the comments sidebar">
<figcaption>The annotation window, with highlights and threaded comments.</figcaption>
</figure>
</div>

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/LibrePaper/librepaper/main/deploy/install.sh | sh
```

The installer supports Linux and macOS. Windows binaries are available on the
[releases page](https://github.com/LibrePaper/librepaper/releases).

## Web interface: Try it now!

The LibrePaper sandbox is a free website where anyone can upload small (<4MB) short-lived (<24hrs) HTML or Markdown files. To upload a document, you will need to log with your Github username:

[LibrePaper sandbox](https://librepaper.arelbundock.com)

If you do not want to log in but want to try annotating some documents, you can try one of these live examples:

- [Markdown: What a Regression Table Is Hiding](https://librepaper.arelbundock.com/docs/markdown-what-a-regression-table-is-hiding-c9kqgt7acs)
- [Typst: What a Confidence Interval Does Not Say](https://librepaper.arelbundock.com/docs/typst-what-a-confidence-interval-does-not-say-5vvxv8ebpd)
- [HTML: What the Bootstrap Actually Resamples](https://librepaper.arelbundock.com/docs/html-what-the-bootstrap-actually-resamples-g6zm9dbzpa) (rendered by Quarto)
- [LaTeX: What a Standard Error Assumes](https://librepaper.arelbundock.com/docs/latex-what-a-standard-error-assumes-75x2fwzpc8)
- [Publication and management console](https://librepaper.arelbundock.com) (requires Github Login)

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. The seeded examples above
are the exception: their suffix is derived from the title rather than drawn at
random, so re-seeding the sandbox leaves these links pointing at the same
documents. That is safe only because an example is public on purpose; every
other document keeps an unguessable address. A link that resolves to nothing
gets a 404 page saying so.

<aside class="callout warning">
<strong>Warning:</strong> Do not publish confidential information on the LibrePaper sandbox. Normally, documents are only visible to the person who uploaded them, or to people holding a share link they minted. But if you are gathering comments on documents about national security, you should probably <a href="#self-managed-server">host your own instance</a> or find another solution.
</aside>

<br>

The standard web-based workflow is:

1. Open a LibrePaper server in a browser, 
2. Sign in with GitHub (if the manager requires it), 
3. Upload an `.html` or `.md` file,
4. Send the read link to your readers, or mint a comment link and send that.

Only somebody holding a live link can open the document; its bare URL opens
for you alone. The LibrePaper console lists only the documents you own or have
been let into.

Click on the thumbnails near to top of this page for screenshots of the LibrePaper management console and annotation page.


## CLI

Every command executed from the CLI must point to a specific LibrePaper server. Typically, users will specify their server with a flag. For example, to make a request against the LibrePaper sandbox, a live instance maintained by the developers, use:

```sh
librepaper <COMMAND> --server https://librepaper.arelbundock.com
```

When making repeated calls to the same server, it is convenient to specify the address using an [Environment Variable](#environment-variables). This allows us to omit the `--server` flag:

```sh
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"

librepaper <COMMAND>
```

In the examples below, we use the environment variables and omit the flag.

### Authenticate

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

### Publish

Publish an HTML or Markdown document:

```sh
librepaper publish paper.html --title "My Paper"
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

Publishing a single file that reads its neighbours says so rather than
publishing a document that compiles here and nowhere else. A reader renders
it themselves, and would get the error you never saw:

```
paper.typ reads lib.typ and refs.bib; publish the directory to send them along:
  librepaper publish .
```

### List

List the documents visible to your account. Each row shows a short ID (at
least three characters, and the same width for every document) along with its
date and title. The ID is a prefix of the document's suffix, so a document
you publish again gets a new one; the seeded examples below keep theirs,
because their suffix is derived rather than random:

```sh
librepaper list
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
librepaper share c9k                       # print the owner's link and every role's link
librepaper share c9k --link comment        # mint (or rotate) the comment link
librepaper share c9k --link edit --until 30d
librepaper share c9k --link comment --label "Review bot" --budget 20
librepaper share c9k --revoke edit         # turn the edit link off
```

That prints one URL with a key in its fragment. A fragment is never sent to a
server, so the key lands in no access log and on no `Referer` header. Minting a
role's link again rotates it: the old key dies and the new one takes over,
which is how a leaked link is killed without losing the role it stood for.
Links expire after six months unless `--until` says otherwise (`--until never`
for one that does not). `librepaper publish` mints the read link when it creates
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
per-document pseudonym such as `AmberAgama-a3f2`, shown next to their comments
instead of a name they typed.

A stranger -- anyone with the URL and no live link -- is answered exactly as
a deleted document answers. The reading frame is served from a separate
documents host that holds no sign-in of yours, so the reader fetches a
short-lived token on the origin that does and puts it on the frame's URL;
that is what lets an HTML document's own scripts run for whoever may read it
and for nobody else.

Named grants -- an editor or a commenter added by GitHub login, from before
links existed -- are legacy: still honoured, still revocable by that login,
but a document never grows new ones. `librepaper share c9k` lists any that remain
under a `people (legacy)` heading.

Every command that acts on one document takes `--key` with a link, or the key
out of one, and then acts as that link's holder rather than as your sign-in:

```sh
librepaper comment c9k --key 'https://librepaper.example.org/docs/c9k#k=…'
librepaper sync c9k paper.typ --key …      # an edit link; no login needed where
                                        # the deployment asks for none
librepaper export c9k --key …
```

The owner is one account, because the storage quota and `destroy` both need an
answer to "whose". Handing it on is its own command, confirmed the way
`destroy` is:

```sh
librepaper transfer c9k alice
```

The document, its history, its comments and its storage quota all move. An
editor cannot share: the owner is the one whose quota and whose name are on the
document. In the browser, all of this is the **Share** button in the reader,
and `librepaper list` marks the documents shared with you with the role you hold.

### Comment

Open a document in your browser for commenting. The ID is the one `list`
prints (a full slug also works), and `--key` takes a link you were sent:

```sh
librepaper comment c9k
```

### Edit

A document published from markdown or typst keeps that source, so it can be
edited in the page it is read in: the source on one side, the document as it will be
published on the other, and the comments beside both. Either of the two panes
next to the source folds away. `edit` takes the same ID `comment` does, and
just opens that page:

```sh
librepaper edit c9k
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
document, readers see it a moment later, and the comments survive it. As you
type, they re-anchor against the edited text. A comment records its passage in
the source file as well as on the page, so it can be found in any version and
in the editor, and one whose passage is gone from both is marked as such
rather than quietly dropped.

Several people can edit at once. The source is a CRDT (Yjs), so two people
typing in the same sentence converge without either waiting for the other, and
the toolbar says how many are in the session. The server holds the document,
relays every update and keeps the result, so closing the last tab loses nothing
and whoever opens the document next, in a browser or with `librepaper sync`, joins
what is there.

What is shared is the source. The preview is not: each browser renders what it
now has, so a session costs the deployment no CPU and no bandwidth beyond
relaying a few dozen bytes per keystroke.

History is kept for you. The server takes a checkpoint of the source when the
document has been quiet for a while, when the last editor leaves, when someone
comments, and whenever `librepaper publish` writes to it. Unchanged text reuses its
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
| **Markdown** | `librepaper publish paper.md` | comrak | ~130 KB compressed |
| **Typst** | `librepaper publish paper.typ` | typst | ~13 MB compressed |
| **HTML** | `librepaper publish paper.html` | the identity | nothing |
| **LaTeX** | `librepaper publish paper.tex` | the browser engine, fetched by the browser | ~6 MB for pdfTeX and its format, then the packages a document asks for |

Both renderers are the same crate the binary itself renders with, compiled to
WebAssembly. Nothing else has to be installed: publishing a `.typ` file needs
no `typst` binary on your PATH, because the compiler is inside LibrePaper, and it
is the same one the editor runs, so a document cannot render one way when it
is published and another way when it is edited.

The Typst module contains the compiler and embedded fonts. Renderer URLs include
their content digest and are cached for a year.

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

The Typst renderer is fetched with the other pinned browser modules as part of
`make build`, so every deployment serves the same four renderer interfaces.

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

`librepaper publish paper.tex` stores a `.tex` file as `latex`, and
`librepaper publish paper/` takes the whole directory: the chapters, the `.bib`,
the figures. Nothing is compiled on the way: LibrePaper carries no TeX, no build
embeds one, and there is no `make latex`.

LaTeX is compiled in the browser, by LibrePaper's own pinned release of the
browser engines: pdfTeX, XeTeX and BibTeX built for WebAssembly, with
the formats generated for those exact binaries and a pinned TeX Live package
set. An editor's browser
loads the engine the project needs the first time it opens a LaTeX document
and compiles automatically from then on; readers see the stored PDF and fetch
no compiler at all. Packages arrive as verified, content-addressed bundles
from the deployment's mirror as a compile asks for them, and stay in browser
storage so the next document costs nothing to fetch. The TeX engines carry
their own licences, and Biber is AGPL-3.0. They are fetched at run time;
their notices travel with the mirror.

The project engine (Automatic, pdfLaTeX, XeLaTeX or LuaLaTeX) and the pinned
browser release are project settings in the Settings panel. Automatic honours
a `% !TEX program = xelatex` line in the main file, then looks for packages
that only a Unicode engine can load, and otherwise uses pdfLaTeX. LuaLaTeX
remains in the selector for release compatibility, but selecting it with the
current release reports that it is not available in this release.

BibTeX and Biber run in the browser when the release provides them. Biber
documents use the release's bundled biblatex pairing; releases without a
Biber engine fall back to the local app and then the browser VM.
In that fallback flow, if LibrePaper's local app is running, the reader hands it the
`.bcf` and the `.bib` files, runs your own Biber, and continues typesetting
in the browser. Without the app, a deployment configured with `--biber-vm` boots a small Linux guest in a
worker (a Debian image holding Biber and nothing else, run by v86) and runs
the real Biber there; slower, but nothing to install.

The local app is the same binary:

```sh
librepaper local start        # a loopback service; prints a pairing code
librepaper local doctor       # which TeX tools it found, and whether it can confine them
librepaper local status
librepaper local disconnect --all
```

`local start` takes `--code` to fix the pairing code instead of a fresh random
one each run, and `--tex-path` (colon-separated directories) when a TeX
installation lives somewhere `start` and `doctor` would not otherwise search.

Enter the code once in the document's Settings panel and later fallbacks are
automatic. When browser compilation fails outright (an engine that will not
start, a package the mirror lacks, a crash, a TeX error), the reader asks the
app to compile the whole project natively with your installed TeX, once per
version of the source, and shows the result as an ordinary preview that says
it was made locally. The app accepts structured jobs rather than commands,
runs the tools with shell escape off, confines them with `bwrap` or
`sandbox-exec` where the platform has them, and says so when it cannot. It
never installs packages or changes your TeX installation.

A self-hoster says where the browser distribution comes from:

```sh
# in the wasm-latex repository: build and push the mirror
make mirror
make push
# back here: use that mirror
make deploy LATEX=../wasm-latex/mirror              # use that mirror locally
librepaper serve --latex /srv/librepaper/latex          # or a copied mirror
librepaper serve --latex https://bucket.example.com  # or a bucket serving one
librepaper serve                                     # LibrePaper always serves LaTeX; with
                                                      # no --latex this defaults to the
                                                      # project's own mirror,
                                                      # https://latex.librepaper.workers.dev/
```

`make deploy` checks that the selected mirror contains a default engine
release with its TeX Live bundles (`latex/tools/check-mirror.mjs`; see
`make latex-check` and `make latex-smoke`, MIRROR=). Older per-file mirrors
and SwiftLaTeX/BusyTeX releases are rejected as legacy. `make latex-smoke` compiles `examples/standard-errors.tex`
in a fresh Chromium profile against MIRROR= and requires visible PDF pages
and selectable text before you point a deployment at it.

LibrePaper always serves the LaTeX editor and compiler. Browsers only ever fetch `/latex/` on
your own origin: the server reads from the bucket, the browser never does,
because the list of packages a document asks for is a description of the
document and should go no further than the deployment that already has the
source. An `http:` mirror is refused at startup, since the page a document is
framed in will not load a compiler over one.

### Typst packages and fonts

A Typst document may import a package from
[Typst Universe](https://typst.app/universe) and name any font family. The
compiler, in the browser and in `publish` alike, never fetches anything
itself: it says what it went looking for and did not find, the host fetches
that, and the compile runs again. Packages come straight from the registry --
a published version never changes, so the browser caches each forever and the
command line keeps them where the `typst` binary keeps its own, under
`~/.cache/typst/packages`. A font file beside the document is used as
`--font-path` would use it.

Any other family comes from the deployment's own font library:

```sh
librepaper serve --fonts /srv/librepaper/fonts      # a directory of .ttf/.otf files
```

The directory is read once at startup for the families each file carries, and
served by family at `/api/fonts/`. The editor asks for a family the compiler
warned about; `publish` asks the same deployment for the same files, so the
preview and the stored PDF are set in the same faces. Without `--fonts`, a
document naming a family the compiler does not embed is set in Typst's
default faces and warned about, as it would be by the binary on a machine
without that font. Which fonts a deployment offers, and under what licence, is
the operator's decision.

### Sync

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
so it asks for a checkpoint in the timeline -- a burst of saves is one mark,
and saving text the document already has is none. If the file is not there
when you start, it is written from the document, which is how you pull one
down to edit locally.

Beside a Makefile that runs `quarto render`, `librepaper sync c9k paper.html`
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

### Bibliographies

Add or upload a BibTeX or BibLaTeX file through Files, or include it when publishing
a directory. In the source editor, type `@` (or `\cite{` in LaTeX) to search by
citation key, author, title, or year. Selecting a result inserts its key.

Markdown accepts Pandoc citations such as `[@smith2020]`, `@smith2020`,
and `[see @smith2020, pp. 3-4; @jones2021]`. Choose resources and a built-in
citation style in front matter:

```yaml
---
bibliography: references.bib
bibliography-style: apa
---
```

With no resource declaration, all `.bib` files in the document form the library.
The reference list appears at a References heading or at the end. Missing keys,
malformed entries, and missing files appear in Diagnostics. Parsing and Markdown
formatting run in separate, lazily loaded WebAssembly modules. LaTeX and Typst
keep their own bibliography compilers. Zotero exports can be uploaded as
`.bib` files; live Zotero integration is a later milestone.

### Math

Markdown accepts TeX between dollars: `$\hat\beta$` in a sentence, and
`$$…$$` on lines of its own for a displayed equation. The renderer keeps the
TeX exactly as written, and the reader typesets it with KaTeX, served by the
deployment itself and fetched only by a document that has math in it. A
dollar with a number after it, as in `$5`, is still money. Typst and LaTeX
typeset their own mathematics.

### Agents

Give an agent a LibrePaper link and it can work on the document with that link's
permissions. A read link reads, a comment link also annotates, and an edit
link also changes source. Signing in supplies attribution and satisfies the
deployment's sign-in policy; it does not give an agent using a read link the
owner's editing rights.

LibrePaper ships three agent skills. Install them with:

```sh
npx skills add LibrePaper/librepaper
```

That works for Claude Code, opencode, Cursor, and the other agents
[`skills`](https://github.com/vercel-labs/skills) supports; pass
`--agent claude-code` to pick one. You can also copy the directories under
[`skills/`](skills) into your agent's skill directory by hand.

- [`librepaper-document`](skills/librepaper-document/SKILL.md): read, comment on,
  and edit a document from its link.
- [`librepaper-pair`](skills/librepaper-pair/SKILL.md): pair live in the sidebar
  chat.
- [`librepaper-write`](skills/librepaper-write/SKILL.md): proofread, tighten, rewrite,
  and explain with anchored suggestions an editor reviews.

Each explains how to install the single LibrePaper binary locally and use its
commands. Any agent that can run commands can use them:

```sh
librepaper agent capabilities 'https://librepaper.example.org/docs/paper#k=YOUR_KEY'
librepaper agent read 'https://librepaper.example.org/docs/paper#k=YOUR_KEY'
librepaper agent comment 'https://librepaper.example.org/docs/paper#k=YOUR_KEY' \
  --exact 'selected words' --body 'Please explain this assumption.'
```

Each command returns JSON. Read the current source before editing, save the
revised text locally, and pass the source SHA you read:

```sh
librepaper agent edit "$LIBREPAPER_DOCUMENT" --file revised.md --expected-sha SOURCE_SHA
librepaper agent checkpoint "$LIBREPAPER_DOCUMENT"
```

`--file` names the local input; `--path` selects a remote file inside a
project. A stale SHA refuses the edit so the agent can read again and account
for other people's changes. Edits synchronize through the same collaborative
session as the browser and wait for durable acknowledgement.

The robot icon opens the assistant panel. **Copy setup prompt** gives your
existing agent instructions to start a local runner. The runner owns a separate
Codex session and keeps its connection open while the model works. Codex must
be installed and authenticated locally; LibrePaper does not receive its model
credentials or run inference on the document server.

```sh
librepaper agent connect "$LIBREPAPER_DOCUMENT" \
  --conversation "$LIBREPAPER_CONVERSATION" --background
```

The setup prompt supplies `LIBREPAPER_CHAT_TOKEN` separately from the document
link. Both are required: a conversation token does not widen document access.
The browser reports queued work, progress, completion, and failures. Follow-up
requests can be submitted while a task runs; cancellation requests stop active
work where supported and never undo document changes already made.

Select a passage to Tighten, Rewrite, or Explain. Address a comment from its
thread, or ask the assistant to fix a diagnostic. Requests carry their captured
source context and revision. Suggestions use the ordinary review interface,
with Accept, Reject, and Refine; refinement updates the existing proposal and
refuses changes to a proposal that was already decided or modified elsewhere.

Focused tools let the assistant inspect files, headings, source sections,
passages, comment threads, bibliography source, and changes since a checkpoint:

```sh
librepaper agent inspect "$LIBREPAPER_DOCUMENT" headings
librepaper agent inspect "$LIBREPAPER_DOCUMENT" search 'selected words'
librepaper agent inspect "$LIBREPAPER_DOCUMENT" thread COMMENT_ID
```

Candidate verification uses the browser's renderer on temporary source files.
It does not apply the candidate to the collaborative document. A render result
belongs to that candidate revision; unavailable renderers or a disconnected
browser cannot produce a successful verification.

The runner keeps task continuity and writing preferences locally. The browser
keeps its assistant history on the same device. The document server relays
messages without storing transcripts. Reconnection reconciles known task IDs;
it does not blindly repeat uncertain work. Use the panel's New conversation
control to clear its history and start a fresh channel, and the runner's
`status` and `stop` commands to inspect or end the local process.


See the [assistant protocol](docs/protocol/chat.md) and [document operations](docs/protocol/room-v1.md).

### History

A document is never lost, and its past is never rewritten. The server takes a
checkpoint when the typing stops, when the last editor leaves, whenever
somebody comments, and whenever the document is published to; each one records
the whole directory at that moment, so a chapter and the file that includes it
can never come back out of step.

```sh
librepaper history c9k
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
librepaper label c9k 4f2a91c "sent to the journal"
librepaper label c9k 4f2a91c            # and to take the name off again
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
librepaper diff c9k 8b03d77 4f2a91c
librepaper restore c9k 8b03d77
```

Restore requires editor access. It records the current version before applying
the earlier directory through the shared editing session, then records the
restore in history. Both versions remain available, subject to the deployment's
history quota. Use `--key` with a share link on either command.

### Suggestions

A suggestion is a comment that proposes a replacement for a passage, inert
until an editor decides it. Propose one by naming the passage rather than a
position, so it still finds its place after the file has moved on underneath
it:

```sh
librepaper suggest c9k --find "with 95% probability" --replace "in 95% of samples"
```

`--find` must occur exactly once in the file (the main file by default;
`--path` names another one); `librepaper suggest` refuses and says how many
times otherwise, so the anchor is never ambiguous. An empty `--replace`
proposes deleting the passage; `--note` adds an optional remark. It prints
the new comment's id, which is what `accept` and `reject` take:

```sh
librepaper accept c9k 22222222-2222-4222-8222-222222222222
librepaper reject c9k 22222222-2222-4222-8222-222222222222
```

`accept` requires editor access. It applies the proposal to the live source
through the shared editing session, the way `restore` does, and records a
checkpoint whose sha it prints; `reject` resolves the suggestion without
touching the document. When the passage has changed too much since the
suggestion was made for the change to land -- even against a three-way merge
with whatever else happened meanwhile -- `accept` exits with status 3 rather
than the usual 1, so a script can tell a stale suggestion apart from an
outright refusal and fall back to the reader's merge editor instead of
retrying blindly. Use `--key` with a share link on all three commands, the
same as `comment`.

### Export

Export annotations as readable Markdown. `export` takes the same ID `comment`
does, a short ID from `list`:

```sh
librepaper export c9k --format markdown --out comments.md
```

Without `--format markdown`, LibrePaper exports W3C Web Annotation JSON-LD.

### Response to reviewers

The one export that is not a list of what was said. `--format response` writes
the document an author has to produce anyway: grouped by reviewer, numbered
within each, with the remark, the passage as that reviewer saw it, what became
of it since, and the thread underneath as the answer.

```sh
librepaper export c9k --format response --since 4f2a91c --out response.md
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
a checkpoint from `librepaper history` and keeps the comments made at or after
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
librepaper destroy --document c9k
```

It deletes the document, its history and its comments. Nothing else on the
server is touched.

## Deploy

LibrePaper is one static binary with everything compiled into it: the reader, the
renderers, and the server. Run it on your laptop for a quick trial, or on a
small host for something durable.

### Local

Run a public local instance with no GitHub setup at all:

```sh
librepaper serve --port 8081 --publishers YOUR-GITHUB-LOGIN
```

Open <http://localhost:8081>. Everything is stored in `librepaper-data` (see [Storage](#storage)).

### Self-managed server

Run the bundled server on your own host, with `--data` set to a persistent
directory:

```sh
librepaper serve --port 8080 --data /var/lib/librepaper --publishers YOUR-GITHUB-LOGIN
```

To let people sign in, set up a [GitHub app](#github-oauth) for this server's address.

Run the server behind a reverse proxy that terminates HTTPS, and have the proxy send the `X-Forwarded-Proto: https` header. That header is how the server knows its own address is an HTTPS one: without it the session cookie is not marked `Secure`, and uploads and comments are refused because the browser's idea of where the page came from does not match the server's. Plain HTTP is fine on `localhost` and nowhere else.

### Retention

Delete documents automatically after their most recent publication:

```sh
librepaper serve --expire-after 24h
```

For a fixed lifetime from the first upload, use `--expire-from created`. Use
`--expire-after never` to disable expiry. Expired documents are removed by an
hourly pass, and once at startup.

### Storage

`librepaper serve` keeps everything in the directory named by `--data` or
`LIBREPAPER_DATA`, `librepaper-data` in the working directory by default: the
catalogue (`catalog.db`), the objects it names, private server state, and the
secrets that keep sessions and share links valid. Back it up if the instance
holds real work; `librepaper backup` writes a verified recovery point of all
of it, and `librepaper restore-backup` restores one into a fresh directory.

Six flags bound what a deployment will store:

| Flag | Caps | Default |
| --- | --- | --- |
| `--max-size` | the texts of one document, and any one rendering of it | 4 MB (maximum 8) |
| `--max-assets` | the figures of one document | 32 MB |
| `--quota` | everything one publisher holds | 100 MB |
| `--storage` | the whole deployment | 5120 MB |
| `--max-documents` | documents one publisher may hold | 50 |
| `--uploads-per-hour` | uploads one publisher may make in an hour | 30 |

```sh
librepaper serve --max-size 8 --max-assets 16 --quota 500 --storage 10240
```

`--max-size` may not be set above 8 MB. It bounds the text a person can see;
what has to be durably saved is the CRDT snapshot behind that text, which
carries the document's edit history and metadata as well, and this deployment
supports snapshots up to 16 MB. A document can therefore reach that second
ceiling without its visible text ever approaching the first -- an edit refused
for that reason says so, and says that the history counts too. A configuration
whose ceilings could accept work the journal could not durably save is refused
at startup rather than at the first save.

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
librepaper serve --publishers alice,anne@example.org --commenters @example.org
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

Authentication hardening updates browser and terminal credentials to separate,
versioned signatures. After upgrading from unversioned credentials, sign in
again in the browser and run `librepaper login` for each terminal. Existing
anonymous visitor cookies retain their ownership and upgrade on the next page
visit. Logout removes the browser cookie; account session revocation is what
invalidates copies of issued credentials.

Google sign-in accepts verified Gmail addresses and Google Workspace accounts
whose hosted domain matches the email domain. Third-party addresses registered
with a personal Google account are refused because Google cannot establish
current ownership of those addresses. Use a supported Google account or GitHub.
The legacy `anygithub` policy remains restricted to GitHub; use `any` to admit
accounts from either provider. Allowlist contents appear in operator startup
logs; ordinary API responses show only a summary.

Device sign-in is served by the single local deployment process. Pending codes
do not survive a restart. The deployment writer lock prevents simultaneous
local servers.

### GitHub OAuth

A server that asks anyone to sign in needs at least one OAuth client of its
own, GitHub's or Google's. A server where both `--publishers` and
`--commenters` are `anyone` never asks, and runs without either.

Create the app at [github.com/settings/developers](https://github.com/settings/developers)
(New OAuth App). Point its two URLs at the server's own address: the
public HTTPS address it sits behind, with the same `/auth/callback` path:

```text
Homepage URL:               https://docs.example.org
Authorization callback URL: https://docs.example.org/auth/callback
```

Pass the client id to `serve` with `--github-client-id`, or its environment
variable `LIBREPAPER_GITHUB_CLIENT_ID`; the secret is environment only, since
an argument is visible in `ps` to every process on the machine and an
environment variable is not:

```sh
export LIBREPAPER_GITHUB_CLIENT_SECRET="..."
```

Readers can sign in with Google instead, or as well: create a *Web application* client at [console.cloud.google.com](https://console.cloud.google.com) under *Credentials*, with the authorised redirect URI set to this server's address plus `/auth/callback/google`, and pass its id with `--google-client-id` (or `LIBREPAPER_GOOGLE_CLIENT_ID`) and its secret as `LIBREPAPER_GOOGLE_CLIENT_SECRET`. The consent screen asks for the scopes `openid`, `email` and `profile`.[^google-data] All three are non-sensitive, so the app needs no verification review, but **publish the consent screen**: one left in *Testing* admits at most a hundred named test users, and everybody else is turned away at Google's own page.

`librepaper logout` deletes the terminal's local token. It does not revoke a
copy held elsewhere. Rotating the server's session key invalidates issued
browser and terminal credentials across the deployment.

## Environment variables

[^github-data]: LibrePaper requests no GitHub scopes through OAuth. It uses the
GitHub API only to obtain your public login name; it does not collect your email,
repositories, or other profile data.

[^google-data]: LibrePaper reads the verified email address on a Google account,
the hosted domain, the account identifier, and the profile name. The address is what
`--publishers`, `--commenters` and a grant by name are matched against, and
where a retention notice is sent; it is shown to no other reader anywhere.
Other readers see the profile name.

Every command-line option has exactly one flag and one environment variable of
the same name: `--foo-bar` is `LIBREPAPER_FOO_BAR`, and the flag wins when both
are set. `librepaper serve --help` (and every other subcommand's `--help`) is
the reference for the full list. There is no config file.

The exceptions, which are environment only because they are secrets or belong
to the installer rather than the binary:

| Variable | Purpose |
| --- | --- |
| `LIBREPAPER_GITHUB_CLIENT_SECRET` | GitHub OAuth app client secret |
| `LIBREPAPER_GOOGLE_CLIENT_SECRET` | Google OAuth client secret |
| `LIBREPAPER_VERSION` | Version the installer fetches |
| `LIBREPAPER_BIN_DIR` | Installation directory the installer uses |

## Building from source

The application embeds the web build and four prebuilt browser renderers.

| | What it is | Built by |
| --- | --- | --- |
| `web/` | the pages: Svelte, Skeleton, CodeMirror 6, Yjs | bun and vite |
| `crates/librepaper/` | the server and the command line | cargo |

Renderer implementations live in the `wasm-*` repositories. Cargo links their
pinned native libraries, and the browser uses WASM artifacts from the same
release tags. The web build writes into `web/dist`, which the binary embeds;
nothing under that directory is edited by hand.

```sh
make web      # the pages, from web/
make wasm     # all four pinned browser renderers
make build    # dist/librepaper, with the pages and renderers embedded
make test     # rustfmt, clippy and the test suite
```

`make build` needs [bun](https://bun.sh) and Node.js. The four browser
renderers are fetched from the exact tags and SHA256 digests in
`wasm-modules.lock`; `make wasm-check` verifies that those tags also match the
native renderer dependencies without network access. To update one renderer,
name both values explicitly, for example `make wasm-update REPO=wasm-markdown
TAG=v0.2.0`, then review the resulting Cargo and lockfile diff.

`make deploy` runs the normal application locally. Configure an OAuth app in
`.env` (see `.env.example`), then sign in: every new account receives four
private, editable examples, one each in HTML, Markdown, Typst, and LaTeX.
These are your own documents. In **Share**, create a Read, Comment, or Edit
link and open it in a separate browser or private window to try that role.
Use `PUBLISHERS=anyone COMMENTERS=anyone` for links that work without sign-in;
these are the Makefile defaults. An owner opening a link still has owner rights.

Examples are created once per new account, survive restarts, and stay deleted
if you remove them. Existing accounts are left unchanged. `make deploy` never
resets or seeds the shared catalogue. The four starter sources ship inside the
binary; signing in does not require Quarto or a checkout of this repository.

### The look

The pages are [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton supplies
the furniture -- buttons, cards, inputs, tables, dialogs, tooltips, toasts --
and `web/src/styles/theme.css` colours all of it from LibrePaper's own four
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
