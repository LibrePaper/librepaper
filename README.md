# LibrePaper

Publish an HTML or Markdown document, share a link to it, and collect
comments and highlights in real time.

- Highlight passages, suggest edits, and comment on figures
- Multiple people can annotate simultaneously, with live updates
- Publish documents from the CLI; review and manage them in the browser
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

# Privacy

Browsers download the LaTeX compiler distribution directly from the default
project mirror, `https://latex.librepaper.workers.dev/`. The mirror receives
the browser's IP address and the digest-named files it requests. Those
requests can reveal package choices and suggest a document's field or
template. Compiler downloads do not send document source or private input
assets to the mirror.

An operator can host a copy and keep compiler requests on their own
infrastructure by passing `--latex-mirror URL` to `librepaper admin serve`. The URL
must be an HTTPS static mirror with the documented mirror layout and headers.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/LibrePaper/librepaper/main/deploy/install.sh | sh
```

The installer supports Linux and macOS. Windows binaries are available on the
[releases page](https://github.com/LibrePaper/librepaper/releases).

## Web interface: Try it now!

The LibrePaper sandbox is a free website where anyone can upload small (<4MB) short-lived (<24hrs) HTML or Markdown files. To upload a document, you will need to log with your Github username:

[LibrePaper sandbox](https://librepaper.arelbundock.com)

Sign in to receive private, editable tutorials for Markdown, Typst, HTML, LaTeX, and Quarto. Each tutorial contains the same LibrePaper walkthrough in that format.

A published document lives at `/docs/<title>-<suffix>`, where the suffix is
random so the link cannot be guessed from the title. Curated tutorial documents
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

When making repeated calls to the same server, it is convenient to specify the address using an [Environment Variable](#environment-variables). This allows us to omit the `--server` flag:

```sh
export LIBREPAPER_SERVER="https://librepaper.arelbundock.com"

librepaper <COMMAND>
```

Operator commands live under `librepaper admin` (including `serve`, `status`,
backups, seeding, and link-key rotation). `local`, `quarto`, `agent`, and
`skills` remain specialist namespaces for integrations and local tooling.

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

Publish an HTML, Markdown, or Quarto document:

```sh
librepaper publish paper.html --title "My Paper"
```

An HTML file is accepted as source and may be self-contained, with images,
styles, and fonts embedded. For a Quarto project, publish `paper.qmd` and its
declared input files instead of uploading a generated HTML result. This
preserves the source and does not run its code. See [Quarto documents](#quarto-documents)
for browser preview and local rendering.

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

Quarto projects use the narrower [sharing policy below](#quarto-documents),
which also excludes raw data, execution caches, and generated output by default.

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
…  2026-09-11  Learn LibrePaper with LaTeX
…  2026-09-11  Learn LibrePaper with HTML
…  2026-09-11  Learn LibrePaper with Typst
…  2026-09-11  Learn LibrePaper with Markdown
…  2026-09-11  Learn LibrePaper with Quarto
```

### Open

Open a document in the browser using the short ID from `list` (a full slug also
works):

```sh
librepaper open c9k
```

### Share in the browser

A document says who may do what to it. There are four roles, as a ladder, each
including the ones beneath it:

| Role | May |
| --- | --- |
| reader | open the document and read its comments |
| commenter | comment, reply, resolve; delete their own |
| editor | edit the source; delete any comment |
| owner | share, transfer, destroy |

A document is shared with links, not people, and a link is the only way in.
The owner mints and manages read, comment, and edit links in the browser's
**Share** pane. Links can be labelled, given a comments-per-hour budget, set to
expire, rotated, or revoked. `publish` prints the initial read link; use the
browser when a document needs a different link or role.

Each link contains a key in its fragment. A fragment is never sent to a server,
so the key lands in no access log and on no `Referer` header. Minting a role's
link again rotates it: the old key dies and the new one takes over, which is
how a leaked link is killed without losing the role it stood for. Links expire
after six months by default; the browser's Share pane can set a different
expiry, including no expiry. `publish` mints the read link when it creates a
document and prints that, so what it prints is the thing to send; revoke it and
the document is yours alone until you mint another.

A link may have a label, which is only the owner's memo about what the one
role link is for, and a comments-per-hour budget. Every person or machine
holding that link shares its budget, even from different addresses; without a
custom budget, the deployment's ordinary comment limit applies. The label
survives a rotation unless another is supplied, and both values disappear when
the link is revoked. These controls are beside each role in the browser's
**Share** pane.

A read link is read-only, whatever `--commenters` says: the switch is a
ceiling on what a link may carry, not a grant to whoever reaches the document.
An edit link authorizes, and the account attributes: editing requires an
account wherever `--publishers` does, so on a server that names its publishers
the holder of an edit link must sign in as one of them before the link edits,
and until then it only comments. Under `--publishers any` it edits as an
authenticated account. Anonymous commenters still need telling apart, so each gets a stable
per-document pseudonym such as `AmberAgama-a3f2`, shown next to their comments
instead of a name they typed.

A stranger -- anyone with the URL and no live link -- is answered exactly as
a deleted document answers. The reading frame is served from a separate
documents host that holds no sign-in of yours, so the reader fetches a
short-lived token on the origin that does and puts it on the frame's URL;
that is what lets an HTML document's own scripts run for whoever may read it
and for nobody else.

Named grants -- an editor or a commenter added by GitHub login, from before
links existed -- are legacy, but remain revocable by that login. `list` marks
documents shared with you with the role you hold.

Sharing, transfer, deletion, comments, suggestions, decisions, editing,
history, comparisons, restores, and labels are browser workflows. The **Share**,
**Files**, **Comments**, and **History** controls in the reader provide these
operations with the document visible beside them.

### Edit in the browser

A document published from markdown or typst keeps that source, so it can be
edited in the page it is read in: the source on one side, the document as it will be
published on the other, and the comments beside both. Either of the two panes
next to the source folds away.

The **Files** sidebar is a folder tree. Its toolbar creates files and folders
inside the selection, or uploads files from your computer; the **File** menu
at the top of the page offers the same, along with downloads and shortcuts to
the Share and History panels. **Download PDF** or **Download HTML** exports the
currently rendered result to the user's computer; LibrePaper does not retain
that result. **Download project** saves every source and input file as a ZIP. Drag files or
folders onto another folder to move them; drop onto empty space in the sidebar
to move them to the top level. Dropping files or directories from your computer uploads them with
their folder structure. Upload name collisions offer **Keep both** or **Skip**.

The **Outline** sidebar lists headings in the open source file, indented by
section level. Click a heading to open the source pane at that section. The
outline updates as you and your collaborators edit, and supports Markdown,
Quarto, Typst, HTML, and LaTeX files.

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
the status row under the toolbar says how many are in the session. The server holds the document source,
relays every update and keeps it, so closing the last tab loses nothing
and whoever opens the document next, in a browser or with `librepaper sync`, joins
what is there.

What is shared is the source. The preview is not: each browser renders what it
now has. The origin still pays for source and asset transfer, collaboration,
persistence, and history maintenance; these are included in its cost policy.

History is kept for you. The server takes a checkpoint of the source when the
document has been quiet for a while, when the last editor leaves, when someone
comments, and whenever `librepaper publish` writes to it. Unchanged text reuses its
checkpoint; an explicit restore records a new event. The history panel lets you
read earlier versions, compare changes, and restore a whole version or bring
back individual passages in the editor.

Rendering happens on clients. Markdown readers render HTML in the browser;
Typst editors compile PDF or experimental HTML previews in a WebAssembly worker,
and LaTeX readers compile from the configured browser mirror. Generated results
are transient and are never stored by the deployment, browser document store,
or backup. The server synchronizes source and input assets; it does not compile
documents.

The formats, and they are not available in the same places:

| | Published with | Renderer | Over the wire |
|---|---|---|---|
| **Markdown** | `librepaper publish paper.md` | comrak | ~130 KB compressed |
| **Quarto** | `librepaper publish paper.qmd` | Markdown draft; optional local Quarto render | Reuses the Markdown renderer |
| **Typst** | `librepaper publish paper.typ` | typst | ~13 MB compressed |
| **HTML** | `librepaper publish paper.html` | the identity | nothing |
| **LaTeX** | `librepaper publish paper.tex` | the browser engine, fetched directly from the mirror | ~6 MB and requested packages from the mirror; these are not origin transfer |

Both renderers are the same crate the binary itself renders with, compiled to
WebAssembly. Nothing else has to be installed: publishing a `.typ` file needs
no `typst` binary on your PATH, because the compiler is inside LibrePaper, and it
is the same one the editor runs, so a document cannot render one way when it
is published and another way when it is edited.

The Typst module contains the compiler and embedded fonts. Renderer URLs include
their content digest and are cached for a year.

Typst defaults to its paged PDF exporter, preserving page layout, columns, headers,
footers, and typography. The shared PDF viewer supplies selectable text for
comments and highlights. Source navigation matches visible text; generated
text and formulas can have no match. Existing project-file and package
resolution limits still apply.

Use **View → Typst HTML preview (experimental)** for a flowing preview, or
**Typst PDF preview** to check the printed layout. The choice is remembered
for this document in this browser. HTML supports semantic text, tables,
citations, embedded images, and MathML equations, but does not reproduce all
PDF formatting; some templates require HTML-specific show rules. It always
uses the browser compiler, including when Calepin is selected for PDF previews.
HTML previews are transient client results. PDF and HTML exports are generated
again on demand, and a compile error keeps source access available while
showing diagnostics; readers can retry in the browser or use their local
companion.

The Typst renderer is fetched with the other pinned browser modules as part of
`make build`, so every deployment serves the same four renderer interfaces.

A document published as HTML is its own source, and its renderer is the
identity: it is shown as it was published, which it always was, and it opens in
the editor like the other two. That covers everything Quarto, Jupyter and
marimo produce, so the live preview, the co-editing and the comments reach the
documents most papers actually arrive in. A document published before HTML was
a source format needs no republishing: the HTML that is stored is its source.

The `.qmd` is the durable Quarto source. Generated previews remain local to the
rendering client and are never published as retained document artifacts.

#### LaTeX

`librepaper publish paper.tex` stores a `.tex` file as `latex`, and
`librepaper publish paper/` takes the whole directory: the chapters, the `.bib`,
the figures. Nothing is compiled on the way: LibrePaper carries no TeX, no build
embeds one, and there is no `make latex`.

LaTeX is compiled in the browser, by LibrePaper's own pinned release of the
browser engines: pdfTeX, XeTeX and BibTeX built for WebAssembly, with
the formats generated for those exact binaries and a pinned TeX Live package
set. An editor's browser fetches the current release from the configured HTTPS
mirror and compiles automatically; readers render the source on demand.
Packages arrive as verified, content-addressed bundles from that mirror and
stay in browser storage so the next document costs nothing to fetch. The TeX engines carry
their own licences, and Biber is AGPL-3.0. They are fetched at run time;
their notices travel with the mirror.

In the editor, **View → Preview format → HTML** selects a live LaTeXML
preview; **PDF** returns to the printed layout. The choice is remembered in
this browser for this document. HTML conversion runs in a separate WebAssembly
worker and reuses the mirror's verified TeX package bundles. It requires a
mirror release containing the `latexml` engine, built and hosted by
[`wasm-latex`](https://github.com/LibrePaper/wasm-latex). The app contains only
the adapter and preview controls. HTML is a transient reading view. An
unsuccessful edit may leave the current preview visible while reporting
conversion diagnostics; it does not create a stored rendering.

The project engine (Automatic, pdfLaTeX, XeLaTeX or LuaLaTeX) is a project
setting in the Settings dialog. Every compile uses the mirror's current
default release; documents do not pin a browser release. Automatic honours
a `% !TEX program = xelatex` line in the main file, then looks for packages
that only a Unicode engine can load, and otherwise uses pdfLaTeX. LuaLaTeX
remains in the selector for release compatibility, but selecting it with the
current release reports that it is not available in this release.

BibTeX and Biber run in the browser when the release provides them. Biber
documents use the release's bundled biblatex pairing. If browser Biber has an
infrastructure failure, the reader can hand the `.bcf` and `.bib` files to the
local companion and continue typesetting in the browser. Bibliography input
errors are shown directly and are not retried through another backend. If the
companion is unavailable, the reader explains that local Biber is required.

The local companion extends the online editor with the tools installed on
your computer. Documents and collaboration stay in the website. Install the
companion from the document's **Enable local rendering** settings, then use
**Open companion** to launch it. The first connection asks permission for
the named site and document; subsequent connections reuse that permission,
including after restarting the companion.

The companion's local settings page shows discovered tools and connected
documents, lets you revoke access, and offers **Start at login** and **Quit
companion**. Startup at login is optional. A browser may separately ask for
permission to connect to a local service; allow that for the LibrePaper site
you use. Compilation permissions do not grant the website access to these
local management controls.

The companion is the same binary as the CLI. Terminal users can still use:

```sh
librepaper local launch       # run in the background
librepaper local start        # run in a terminal; prints a fallback pairing code
librepaper local stop         # stop the background companion
librepaper local startup enable   # optional: start when you log in
librepaper local startup disable
librepaper local doctor       # which TeX tools it found, and whether it can confine them
librepaper local status
librepaper local disconnect --all
```

`local start` takes `--code` to fix the pairing code instead of a fresh random
one each run, and `--tex-path` (colon-separated directories) when a TeX
installation lives somewhere `start` and `doctor` would not otherwise search.

Enter the code once in the document's Settings dialog and later fallbacks are
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
make deploy LATEX_MIRROR=https://bucket.example.com  # use that mirror locally
librepaper admin serve --latex-mirror https://bucket.example.com
librepaper admin serve                                     # defaults to the project mirror:
                                                      # https://latex.librepaper.workers.dev/
```

`make deploy` checks that the selected mirror contains a default engine
release with its TeX Live bundles (`tools/latex/tools/check-mirror.mjs`; see
`make latex-check` and `make latex-smoke`, MIRROR=). Older per-file mirrors
and SwiftLaTeX/BusyTeX releases are rejected as legacy. `make latex-smoke` compiles `docs/examples/tutorial-latex/librepaper.tex`
in a fresh Chromium profile against MIRROR= and requires visible PDF pages
and selectable text before you point a deployment at it.

LibrePaper always serves the LaTeX editor and compiler configuration. Browsers
fetch distribution files directly from the HTTPS mirror; the origin does not
proxy or cache them. A directory path or an `http:` mirror is refused at
startup. The browser verifies each file against the mirror manifest before
use.

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
librepaper admin serve --typst-fonts /srv/librepaper/fonts      # a directory of .ttf/.otf files
```

The directory is read once at startup for the families each file carries, and
served by family at `/api/fonts/`. The editor asks for a family the compiler
warned about; `publish` asks the same deployment for the same files, so the
preview uses the same faces. Without `--typst-fonts`, a
document naming a family the compiler does not embed is set in Typst's
default faces and warned about, as it would be by the binary on a machine
without that font. Which fonts a deployment offers, and under what licence, is
the operator's decision.

### Quarto documents

Publish and synchronize the actual source:

```sh
librepaper publish paper.qmd
librepaper sync <document> paper.qmd
```

The Tools menu offers exactly one active preview mode at a time, remembered
per document (default: **Quarto preview**):

- **Markdown preview** renders the source as Markdown in the browser — front
  matter dropped, `:::` divs and code chunks shown verbatim — and never runs
  code.
- **Quarto preview** runs the document with Quarto on your own computer,
  through the local app.

Rendering runs on your own computer, with Quarto and R or Python installed
there. Enable local rendering from the preview banner, or start the companion
from a terminal:

```sh
librepaper local launch
```

The first time you pick Quarto preview, a small window from the local app
asks whether to allow that site to use this computer's tools; click
**Allow**. Once paired, your edits sync into the app's own workspace, Quarto
re-renders there, and the pane polls the result and paints it in, so comments
and highlights work on the live page too. The previous render stays on screen
while a new one is under way, so figures never flash blank. If the browser
isn't paired, or no local app is available, the pane shows Markdown preview
with a **Connect** button in the banner. These run the document's code,
filters, and scripts on your machine, so allow only sites you trust. The
pairing code the app prints still works as a fallback under Tools, **Local
app settings…**. Nothing rendered is ever uploaded: the server holds only the
`.qmd` source and its declared shared resources.

When `librepaper admin serve` runs on the machine you browse from, it runs the
local app itself: nothing to start. Pass `--no-local` to turn that off.

Publishing a project directory includes editorial resources and code, but skips
generated output directories, execution caches, environments, and raw data by
default. Add exact project-relative paths to `.librepaper-share.json` when an
additional input is intended for collaborators:

```json
{"include": ["data/public.csv"]}
```

Review the publication inventory. Shared source and assets are readable by
collaborators with document access. Files needed only for local execution can
remain in the author's project.

A project that keeps data the document does not share can be linked with
**Choose project folder…** in the local rendering settings. The native folder
dialog runs on this computer, and the selection is remembered for this site
and document. Choosing a folder does not upload its contents. The website
receives an opaque binding identifier, not the folder's absolute path.

Terminal users can make the same explicit link, which takes precedence for
render jobs for the document:

```sh
librepaper local doctor
librepaper local quarto bind https://your-librepaper-server.example <document-slug> \
  --root /path/to/project --main paper.qmd
```

The paired app renders against that project instead of its hosted workspace
for this document. Revoke it with `librepaper local quarto unbind <binding>`.

For parser inspection without execution, use
`librepaper quarto inspect paper.qmd`.

### Typst documents with Calepin

Ordinary Typst rendering — the browser's own compiler, described above — is
unchanged. A Typst document with code chunks can also offer **Calepin
preview** under Tools alongside **Typst preview** (the default): it runs
`calepin watch` on your own computer, through the local app, and shows the
resulting PDF in the usual PDF viewer, where comments work. This needs
Calepin installed on the machine running the local app; pairing works
exactly as it does for Quarto preview —
one **Allow** click, no binding required. If Calepin isn't installed, the
banner says so. Nothing rendered is ever uploaded.

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

LibrePaper bundles three agent skills and their reference files in the binary.
Read them directly, offline, without installing a separate skills package:

```sh
librepaper skills list
librepaper skills show librepaper-document
librepaper skills show librepaper-document --file references/editing.md
```

Updating LibrePaper updates its bundled instructions at the same time. Check
`librepaper --version` and the required commands' `--help` for compatibility;
using bundled skills does not require an online latest-release check. The
sidebar's connection prompt tells the agent how to read them.

For agents that discover skills through directories, export the complete bundle
with `librepaper skills export ./librepaper-skills`, then copy the
desired skill directories into your agent's skill directory. The export target
must be new and its parent must exist; existing files are never overwritten.
Export again to a fresh directory after upgrading. Installing from the repository
with `npx skills add LibrePaper/librepaper` remains an optional alternative.

- [`librepaper-document`](skills/librepaper-document/SKILL.md): read, comment on,
  and edit through the configured MCP tools.
- [`librepaper-pair`](skills/librepaper-pair/SKILL.md): pair live in the sidebar
  chat.
- [`librepaper-write`](skills/librepaper-write/SKILL.md): proofread, tighten, rewrite,
  and explain with anchored suggestions an editor reviews.

The skills teach agents to use LibrePaper's MCP tools. Direct document
operations are deliberately not duplicated as shell commands.

The robot icon opens the assistant panel. **Copy setup prompt** gives your
existing agent instructions to start a local runner. The runner owns a separate
Codex session and keeps its connection open while the model works. Codex must
be installed and authenticated locally; LibrePaper does not receive its model
credentials or run inference on the document server.

```sh
librepaper agent connect "$LIBREPAPER_DOCUMENT" \
  "$LIBREPAPER_CONVERSATION" --background
```

The setup prompt supplies `LIBREPAPER_CHAT_TOKEN` separately from the document
link. Both are required: a conversation token does not widen document access.
The browser reports queued work, progress, completion, and failures. Follow-up
requests can be submitted while a task runs; cancellation requests stop active
work where supported and never undo document changes already made.

The runner configures the Codex thread with LibrePaper's MCP tools. It starts
the bundled stdio adapter as `librepaper agent mcp -`, inheriting the protected
`LIBREPAPER_DOCUMENT` environment value. The model uses `document_read`,
`document_propose`, `document_apply`, `document_comment`, and
`document_result`; document links and tokens are never MCP tool arguments.
Hosts that need a standalone adapter can register the same command with
`codex mcp add librepaper -- librepaper agent mcp -` and provide
`LIBREPAPER_DOCUMENT` in the host's protected environment.

Select a passage to Tighten, Rewrite, or Explain. Address a comment from its
thread, or ask the assistant to fix a diagnostic. Requests carry their captured
source context and revision. Suggestions use the ordinary review interface,
with Accept, Reject, and Refine; refinement updates the existing proposal and
refuses changes to a proposal that was already decided or modified elsewhere.

`document_read` provides focused queries for files, headings, source sections,
passages, comment threads, bibliography source, and changes since a checkpoint.

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

Live saving and retained history are separate. The server takes checkpoints
after editing pauses and at explicit milestones; each retained checkpoint
records the whole directory, so a chapter and the file that includes it can
never come back out of step. Routine recovery points become less dense as
they age. Retention does not delay ordinary live saving.

In the browser's History panel, name a moment so it stands out and is
preferentially retained. Names do not guarantee permanent storage: deployment
count and storage limits still apply.

In the reader, the history button opens the same list beside the document,
newest first, with the live document as the top row. Picking a moment shows
the document as it was then, with what changed since the baseline struck
through and underlined in place, the way a version history does it; the head
of the panel counts the changes and steps through them. A row's compare
action makes that checkpoint the baseline, and a bracket down the timeline
shows the range. "Back to now" returns to the live document. Copying a
checkpoint's link preserves the share key that gave you access. Editors can
name checkpoints and restore earlier versions.

Historical comparisons render captured source as HTML, including for documents
normally viewed as PDFs. They never compile a historical PDF. When rendering
or an exact target mapping is unavailable, the interface offers a source
comparison instead. A checkpoint's actor identifies who recorded the event,
not necessarily who authored every changed passage; uncertain authorship is
shown as unknown.

Signed-in owners can choose a soft history budget, retention density, warning
thresholds, and display timezone in Storage settings. Retention buckets always
use UTC. Material reductions require a preview and confirmation, with a grace
period before routine thinning. Existing histories keep their legacy policy
until the owner applies preferences. These preferences cannot raise the
deployment's hard quota.

The changes are also listed as prose, folded away under the count, and the
files that changed open source comparisons; editors can compare two
checkpoints and bring individual changes into the live source.

The same comparisons and whole-version restore are available from the History
panel in the browser. Restore requires editor access; it records the current
version before applying the earlier directory, so both versions remain
available subject to the deployment's history quota.

### Suggestions

Suggestions are made by selecting a passage in the browser and choosing
**Suggest**. Editors can then refine, accept, or reject them in the comment
thread; stale suggestions are reported there without silently changing the
document.

### Export

Export annotations as readable Markdown. `export` takes the short ID from
`list` (a full slug also works):

```sh
librepaper export c9k --format markdown --output comments.md
```

Without `--format markdown`, LibrePaper exports W3C Web Annotation JSON-LD.

### Response to reviewers

The one export that is not a list of what was said. `--format response` writes
the document an author has to produce anyway: grouped by reviewer, numbered
within each, with the remark, the passage as that reviewer saw it, what became
of it since, and the thread underneath as the answer.

```sh
librepaper export c9k --format response --since 4f2a91c --output response.md
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
a checkpoint selected in the browser's History panel and keeps the comments
made at or after it, which is a round of review.

**Then** is a quotation rather than a recollection, because every comment
records the checkpoint it was made on. **Now** says whether the passage is
still in the document and quotes its replacement when the word diff can
identify it. The line is left out entirely for a
document this machine cannot render -- a LaTeX paper, whose compiler is in a
browser.

### Delete a document

Owners delete a document, including its history and comments, from the
browser's document management controls. Nothing else on the server is touched.

## Deploy

LibrePaper is one static binary with everything compiled into it: the reader, the
renderers, and the server. Run it on your laptop for a quick trial, or on a
small host for something durable.

### Local

Run a public local instance with no GitHub setup at all:

```sh
librepaper admin serve --port 8081 --publishers YOUR-GITHUB-LOGIN
```

Open <http://localhost:8081>. Everything is stored in `librepaper-data` (see [Storage](#storage)).

### Self-managed server

Run the bundled server on your own host, with `--data-directory` set to a persistent
directory:

```sh
librepaper admin serve --port 8080 --data-directory /var/lib/librepaper --publishers YOUR-GITHUB-LOGIN
```

To let people sign in, set up a [GitHub app](#github-oauth) for this server's address.

Run the server behind a reverse proxy that terminates HTTPS, and have the proxy send the `X-Forwarded-Proto: https` header. That header is how the server knows its own address is an HTTPS one: without it the session cookie is not marked `Secure`, and uploads and comments are refused because the browser's idea of where the page came from does not match the server's. Plain HTTP is fine on `localhost` and nowhere else.

### Retention

Delete documents automatically after their most recent publication:

```sh
librepaper admin serve --document-expire-after 24h
```

For a fixed lifetime from the first upload, use `--document-expire-from created`. Use
`--document-expire-after never` to disable expiry. Expired documents are removed by an
hourly pass, and once at startup.

### Storage

`librepaper admin serve` keeps everything in the directory named by `--data-directory` or
`LIBREPAPER_DATA`, `librepaper-data` in the working directory by default: the
catalogue (`catalog.db`), the objects it names, private server state, and the
secrets that keep sessions and share links valid. Back it up if the instance
holds real work; `librepaper admin backup create` writes a verified recovery point of
all of it, and `librepaper admin backup restore` restores one into a fresh
directory.
See the [operator cost policy](docs/cost-policy.md) for the complete defaults,
advanced YAML schema, `admin status` command, capacity accounting, and backup
reservations.

The storage flags bound what a deployment will store:

| Flag | Caps | Default |
| --- | --- | --- |
| `--document-size-limit` | combined source text of one document | 4 MB (maximum 8) |
| `--document-assets-limit` | combined input assets of one document | 32 MiB |
| `--publisher-storage-limit` | everything one publisher holds | 100 MB |
| `--deployment-storage-limit` | the whole deployment | 5120 MB |
| `--publisher-document-limit` | documents one publisher may hold | 50 |
| `--publisher-upload-limit` | uploads one publisher may make in an hour | 30 |

```sh
librepaper admin serve --document-size-limit 8 --document-assets-limit 16 --publisher-storage-limit 500 --deployment-storage-limit 10240
```

`--document-size-limit` may not be set above 8 MB. It bounds the text a person can see;
what has to be durably saved is the CRDT snapshot behind that text, which
carries the document's edit history and metadata as well, and this deployment
supports snapshots up to 16 MB. A document can therefore reach that second
ceiling without its visible text ever approaching the first -- an edit refused
for that reason says so, and says that the history counts too. A configuration
whose ceilings could accept work the journal could not durably save is refused
at startup rather than at the first save.

A document is a directory, so `--document-size-limit` bounds the sum of its texts and
`--document-assets-limit` bounds the combined input assets. Both count against `--publisher-storage-limit`; a figure is
an upload and counts against `--publisher-upload-limit` like any other. A Typst or
LaTeX document keeps source and input assets only. PDF and HTML output created
by a browser or companion is transient and never counts toward storage,
quotas, or uploads. On a deployment with many publishers,
`--document-assets-limit` is the one worth lowering:
figures are where a paper's bytes actually are, and it is what stops a single
document spending a publisher's whole allowance on images.

Origin transfer has its own rolling 24-hour budget:

```sh
librepaper admin serve --transfer-budget 10GiB
```

The value is bytes (binary suffixes such as `KiB`, `MiB`, `GiB`, and `TiB` are
accepted). An explicit `0` refuses ordinary transfer while retaining the small
emergency allowance for control and durability responses. Omitting the flag
keeps transfer unlimited and produces a startup warning. Compiler files come
from the direct mirror and do not count against this origin budget.

Publishing is always attributed to an authenticated Google or GitHub account
and charged against that account's quota. Anonymous readers and commenters do
not receive a publishing quota.

### Rights

Two flags say who may do what.
`--publishers` says who may upload documents, and `--commenters` who may
annotate them. Both accept a comma-separated list of names:

```sh
librepaper admin serve --publishers alice,anne@example.org --commenters @example.org
```

A name is a GitHub login, a Google account's verified email address, or a whole
domain of them; the shape of the entry is what decides which, so the forms mix
freely in one list. `any` admits signed-in accounts; `anyone` is available only
for commenting:

| Value | Meaning |
| --- | --- |
| `alice` | the GitHub login `alice` |
| `alice@example.org` | the Google account whose verified email is that address |
| `@example.org` | any Google account on that domain |
| `any` | any signed-in account, on either provider |
| `anyone` | unsigned-in commenting; rejected for publishing |

A domain matches the part after the `@` exactly, so `@example.org` admits
`alice@example.org` and not `alice@mail.example.org`.

`--publishers` has no default: the server insists you say who may publish.
Publishing requires a Google or GitHub OAuth provider. Use `any` for any
authenticated account, or name specific accounts. The old `anyone` spelling is
rejected at startup.
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

Forwarded client identity is trusted only from networks listed in the advanced
configuration file. A single loopback proxy can use:

```yaml
trusted_proxies:
  - 127.0.0.1/32
  - ::1/128
```

The proxy must append the actual client address to `X-Forwarded-For` and
overwrite any incoming value. Without this setting, the TCP peer address is
used, so visitors behind one proxy share its IP based limits. The server walks
trusted proxy hops from right to left and stops at the first untrusted address.

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

Pass the client id to `admin serve` with `--github-client-id`, or its environment
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
GitHub API only to obtain your public login name and account id; it does not
collect your email, repositories, or other profile data. The bar shows you your
own public avatar, fetched from GitHub by your browser.

[^google-data]: LibrePaper reads the verified email address on a Google account,
the hosted domain, the account identifier, the profile name and the profile
picture. The address is what `--publishers`, `--commenters` and a grant by name
are matched against, and where a retention notice is sent; it is shown to no
other reader anywhere. Other readers see the profile name. The picture is shown
only to you, in the bar, and its address is kept in your own session cookie.

Service settings that support environment variables follow the same name:
`--foo-bar` is `LIBREPAPER_FOO_BAR`, and the flag wins when both are set.
`librepaper admin serve --help` (and every other subcommand's `--help`) is the
reference for the full list. Advanced guardrails may be overridden in an
optional YAML file selected with `--config PATH` (or `LIBREPAPER_CONFIG`). The
proxy list is the top-level `trusted_proxies` key; `cost.trusted_proxies` is
not accepted. An optional `backup` map is reporting metadata for
operator-managed backups (`destination_class`, `frequency` in seconds,
`retained_count`, `encrypted`, and `warning_count`); it does not schedule or
delete backups. Omitted keys retain their documented defaults.

These variables are useful in deployment files. Secrets are environment-only;
service settings have corresponding CLI flags, and installer variables control
the installation script:

| Variable | Purpose |
| --- | --- |
| `LIBREPAPER_GITHUB_CLIENT_SECRET` | GitHub OAuth app client secret |
| `LIBREPAPER_GOOGLE_CLIENT_SECRET` | Google OAuth client secret |
| `LIBREPAPER_BUDGET_TRANSFER` | rolling 24-hour origin response budget; bare values are bytes |
| `LIBREPAPER_BUDGET_DOCUMENT_ASSETS` | combined input assets per document, in MiB |
| `LIBREPAPER_LATEX_MIRROR` | HTTPS static mirror URL fetched directly by browsers |
| `LIBREPAPER_TYPST_FONTS` | optional local directory of additional Typst fonts |
| `LIBREPAPER_CONFIG` | optional advanced YAML policy overrides |
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
make install  # build and install to ~/.local/bin (override PREFIX= or BINDIR=)
make test     # rustfmt, clippy and the test suite
make test-external          # Quarto/R/Python and local-service integrations
make test-release-workloads # supported limits and diagnostic workloads
```

`make build` needs [bun](https://bun.sh) and Node.js. The four browser
renderers are fetched from the exact tags and SHA256 digests in
`wasm-modules.lock`; `make wasm-check` verifies that those tags also match the
native renderer dependencies without network access. To update one renderer,
name both values explicitly, for example `make wasm-update REPO=wasm-markdown
TAG=v0.2.0`, then review the resulting Cargo and lockfile diff.

`make deploy` runs the normal application locally. Configure an OAuth app in
`.env` (see `.env.example`), then sign in: every new account receives five
private, editable examples, one each in HTML, Markdown, Typst, LaTeX, and Quarto.
These are your own documents. In **Share**, create a Read, Comment, or Edit
link and open it in a separate browser or private window to try that role.
Use `PUBLISHERS=any COMMENTERS=anyone` for local development. Publishing still
requires an authenticated account, while comments may remain anonymous.

Examples are created once per new account, survive restarts, and stay deleted
if you remove them. Existing accounts are left unchanged. `make deploy` never
resets or seeds the shared catalogue. The five starter sources ship inside the
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
