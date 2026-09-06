# SPEC: LaTeX, compiled in the browser, from a distribution the reader chooses

Status: the compiler, the mirror and the viewer are built and are no longer
described here. `latex/` builds and serves the mirror, `web/src/lib/latex.js`
and `web/src/lib/latex/` drive four distributions from a worker and read
their logs, `web/src/viewer.js` and `web/src/lib/pdf/` draw a PDF with
comments anchored into its text layer, and `komodoc/src/latex.rs` is
`--latex`. What remains is the reader's side of step 3 -- the card and the
compiling state, which is what puts a compile in front of an author -- then
renderings, SyncTeX and the optional local command. It is written against the
editor `01-SPEC-history.md` describes, where readers render the text
themselves, and it makes one exception to that spec's "nothing derived is
stored", stated and bounded below.

## What the remaining work builds on

| where | what |
| --- | --- |
| `latex/distributions.mjs`, `latex/mirror.mjs`, `latex/serve.mjs` | the four distributions Komodoc drives, as data; the mirror, with digested file names and a `manifest.json` carrying each distribution's engines, licence, measured bytes and `shown` flag |
| `komodoc serve --latex <url-or-dir>` | where the mirror is. Defaults to the project's bucket, refuses plain `http:` at startup, and is served to browsers from `/latex/` on the deployment's own origin, so the list of packages a document asks for goes no further than the deployment that has the source. `/api/config` says `latex: true` when a mirror is configured |
| `web/src/lib/latex.js` | `at(url)`, `available()`, `chosen()`, `choose(name)`, `compile(tree)` returning `{pdf, synctex, log, diagnostics}`. Cache Storage under the mirror's URL, persistence asked for once; at most one compile running and one queued, the queued one always the latest |
| `web/src/lib/latex/worker.js`, `swiftlatex.js`, `busytex.js`, `texlyre.js`, `log.js` | the worker, the glue, one file per distribution, and the log parser, which emits `engine/src/diagnostic.rs`'s shape and treats a line it does not recognise as nothing |
| `web/src/viewer.js`, `web/src/lib/pdf/` | the frame for a PDF, on the documents origin, taking the editor's `preview` message with `pdf` bytes where an HTML document has `html`; every page's canvas and text layer stacked in one scrolling document, the agent's table rebuilt as pages render, highlights painted in the text layer; `komodocViewer.pageForOffset` exposed for SyncTeX |
| `examples/latex/` | the corpus, its logs per engine, `MEASUREMENTS.md`, and `pages.json` from a TeX Live on a desk |
| `web/scripts/latex-log-check.mjs`, `latex-check.mjs`, `viewer-check.mjs` | the parser over every log in the corpus; the distributions in headless Chromium against the mirror; the anchoring miss rate over a hyphenated line end, a page break inside a sentence, a footnote and a ligature |

Of the four distributions only SwiftLaTeX pdfTeX is `shown`. The
measurements found that SwiftLaTeX XeTeX's dvipdfmx has no font to embed and
that BusyTeX cannot load `fontenc`; TeXlyre's TeX Live 2026 BusyTeX is
measured and not yet offered. The flag travels in the manifest, so offering
another is an edit to `distributions.mjs` and a rebuild of the mirror, with no
build of Komodoc involved. The card lists what the manifest marks shown and
names none itself.

The mirror carries TeX Live's `latex-recommended`, `latex-extra`,
`fonts-recommended` and `mathscience` collections, fetched one file at a time
by name. `tikz` and `biblatex` are not in it, and SwiftLaTeX's preloaded
format is LaTeX2e 2020-02-02, which `siunitx` refuses; either failure is the
engine's own error, in the badge and the gutter that typst errors use.

A `.tex` file publishes with format `latex`, `source_formats` has `latex`,
and the server compiles nothing: no TeX ships in the executable, and there is
no `make latex`. `max_html` is `max_document`, because it bounds a PDF now.

## The problem

Komodoc renders markdown and typst because both are Rust crates: the engine
compiles them to WebAssembly, the browser previews with that module, and the
command line publishes with the same crate built natively. A `.typ` file
needs no `typst` binary on anyone's PATH, and the preview cannot disagree
with the publish, because there is one compiler.

Most of the scientific writing Komodoc is for is not written in either. It
is written in LaTeX, and LaTeX cannot take the typst road. TeX is C, not a
crate; a working distribution is TeX Live, which is gigabytes, not a font
directory; and the engine's HTML export -- the text nodes that let comments
anchor at all -- has no TeX counterpart worth having. Embedding a TeX
distribution in the binary the way `make typst` embeds thirty megabytes is
not a bigger version of the same thing. It is a different thing, and the
binary is the wrong place for it.

The browser is not. TeXlyre demonstrates this today: an open-source,
local-first web editor for LaTeX and typst that compiles both in the page.
For LaTeX it drives WebAssembly builds of the TeX engines -- SwiftLaTeX's
ports of pdfTeX and XeTeX, and BusyTeX, which packs TeX Live with pdfTeX,
XeTeX and LuaTeX into one module -- and for typst it uses typst.ts. The
compilers exist, they run in a worker, and a document of ordinary size
compiles in seconds. What Komodoc had to decide was not whether a browser
can compile LaTeX but where the compiler comes from, what it produces, and
how a reader who never compiles anything sees the result.

The Overleaf research note (`market-research/`, §1 and §2) sets the bar a
LaTeX editor is measured against: a compile-preview loop with a PDF in an
embedded viewer, parsed errors with jump-to-line, and SyncTeX between the
source and the page. Nothing there requires a server to compile; Overleaf
has one because it predates WebAssembly, not because the problem needs it.

## The decision

Four rules.

**The compiler is not in the binary. It is in the browser, downloaded once,
when a reader asks for it.** The first time a LaTeX document is opened in a
browser, the preview pane is empty and shows a card offering the
distributions the manifest marks shown, each with its engines, its download
size and what it can and cannot do. Nothing is fetched until one is chosen.
The chosen distribution is stored in that browser and is not asked for
again, for that document or any other; the card is reachable afterwards
from the preview toolbar to switch.

**The document is a PDF, and comments anchor into its text.** LaTeX makes
pages. The preview is the PDF the compiler produced, drawn by pdf.js in the
same frame the HTML documents use, and pdf.js's text layer -- real text
nodes positioned over the page -- is what the agent walks, so a highlight
on a LaTeX document is the same text-quote anchor as a highlight on a
markdown one. No conversion of LaTeX to HTML is attempted, here or later.

**Readers do not compile.** A reader who opens a LaTeX document sees a PDF
that an editor's browser compiled, stored beside the checkpoint it was
compiled from and named by that checkpoint's SHA. This is the one exception
to `01-SPEC-history.md`'s rule that nothing derived is stored, and it is
allowed because a rendering keyed by the SHA of its source cannot disagree
with that source silently: either the live text has that SHA and the
rendering is current, or it does not and the reader is told so. Asking a
reader to fetch a TeX distribution to read a paper is not a trade this
project makes.

**A compile is slow and says so.** The typst preview repaints sixty
milliseconds after the last keystroke because the engine takes single-digit
milliseconds. A LaTeX compile takes seconds, in a worker, and a preview that
started a compile on every pause would spend the whole session behind. So
the debounce is longer, the pane says a compile is running and how long the
last one took, the last page that compiled stays up while the next one
runs, and errors come from the log parsed into the diagnostic shape
`engine/src/diagnostic.rs` defines, so the underline and the gutter mark are the
same ones typst errors get.

Markdown and typst documents are untouched by everything below. The format
is `latex`, the file extension `.tex`, and every surface here is inert for
any other format.

## The preview

The preview pane in a LaTeX document has three states, and the frame is the
same frame with the same agent in each.

**No distribution.** The card. It lists the distributions `available()`
returns with engine, licence, size and the one-line trade, with the one
`chosen()` names preselected if any. The size it shows is the up-front bytes
plus what the first document fetched, which is the honest number and the
one `MEASUREMENTS.md` explains. It also says what "chosen" means: a download
of that size, once, kept in this browser, and that a browser which refused
persistence may fetch it again some day. Choosing calls `choose` and shows
a progress bar over the download in the pane; the editor is usable
throughout. For a reader who may not edit, this state does not occur: they
see "not yet rendered" and the source if they want it, because a reader is
never asked to download a compiler. On a deployment whose `/api/config` says
`latex: false`, nobody sees the card; the reader offers the source.

**Compiling.** The last PDF that compiled stays up, dimmed by nothing; the
badge says "compiling" with a spinner and, after the first compile, the
seconds the last one took. On failure the badge says how many errors, as
for typst, and the last page stays. A compile starts after the source has
been quiet for `1500` milliseconds -- twenty-five times the typst delay and
still well under the compile itself -- and the delay is a constant in
`latex.js`, not a setting, because a setting nobody changes is a lie in the
docs. The raw log is one click from the badge, for whatever the parser did
not catch.

**Rendered.** The PDF in the frame, which is the viewer page. The editor
sends it the `preview` message with the bytes `compile` returned, the way it
sends HTML today; everything after that message is built.

## Renderings, stored

A rendering is stored at `renderings/<slug>/<sha>`, where `<sha>` is a
checkpoint's SHA in the sense of `01-SPEC-history.md`, and its bytes are the
PDF. Beside it, `renderings/<slug>/<sha>.synctex` holds the SyncTeX file,
gzipped as the compiler wrote it. Both are readable by anyone who may read
the document, like the checkpoint itself.

An editor's browser stores a rendering by `PUT`ting the PDF to
`/api/documents/<slug>/renderings/<sha>` after a successful compile. The
server accepts it when the caller may edit the document, when the SHA is a
checkpoint or the SHA of the live text -- in which case it takes a
checkpoint first, the way a comment does -- and when the size is within
`max_document`. A rendering counts against its owner's quota. It is derived,
so unlike a checkpoint it may be removed: a rendering is kept for the newest
checkpoint that has one and for every labelled checkpoint, and the server
prunes the rest on the retention schedule. That keeps the cost per document
at one PDF plus one per name the author gave, and on the sandbox, whose cost
is the first constraint of every spec here, a PDF of a few hundred kilobytes
for an hour is what a checkpoint of the source already is, ten times over.

The browser does not store a rendering after every compile. It stores one
when the compile succeeded and the text it compiled is quiet -- the same
five-minute quiet the server takes a checkpoint after, observed
client-side as "no edit since the compile finished and none for a minute
after" -- and on the editor's own act of naming a checkpoint, which is
when an author means "this one". Between those, readers see the newest
rendering there is, with a line in the badge saying it was rendered from an
earlier version and when, and the current source is theirs to look at. An
editor with the pane open sees every compile, because that is what a
preview is.

A document with no rendering at all -- one just created by `komodoc
publish paper.tex`, before any browser has opened it -- shows a reader
"not yet rendered", and shows the first editor to open it the card or, if
their browser has a distribution, the first compile. The command line
stores the source with format `latex` and renders nothing: no TeX ships in
the executable, and this spec does not put one there. Optional local CLI
compilation is a later step. First choose and test a local runner for the
same browser compiler artifacts: the Rust executable does not supply the
JavaScript environment the Emscripten glue needs. A separately installed
runner may be required for that optional command, but never for `serve`,
source-only publishing, or reading. The distribution cache belongs on the
client machine, and no compilation is moved to the deployment.

## SyncTeX

The reader already has a place-mapping between the source and the page for
markdown and typst, `sync.sourcePlaceFor`, built on the text the engine
emits. For a PDF the mapping is SyncTeX's, and the compiler writes it for
free where the engine can: `compile` returns `synctex` from TeXlyre's
BusyTeX, whose pipeline runs with `-synctex=1`, and `null` from SwiftLaTeX,
whose modules export no SyncTeX at all, and from BusyTeX, whose pipeline
does not ask for one. So this step waits on a distribution with SyncTeX
being shown, or on SwiftLaTeX being rebuilt with it, and the gestures below
are inert when `synctex` is null.

The `.synctex.gz` is parsed in the viewer into two tables, source line to
page and box, and page position to source line, and the two existing
gestures are wired to them: the caret's line scrolls the frame to its box
and outlines it for a moment, and a double-click on the page moves the
editor's caret to the line. `komodocViewer.pageForOffset` is the viewer's
half of the first. Neither is needed for reading and commenting, which is
why they are the last step and not the first; both are what the research
note calls table stakes for a LaTeX editor, and both are a parse of a file
the compiler already produces.

## Steps

3. **The card and the compiling state.** The preview pane's three states in
   the reader, drawn from `available()` and `chosen()`; the debounce, the
   badge, the last-page-stays rule; the `preview` message with the bytes to
   the viewer. The reader opens a `.tex` document in the editor with the
   card in the pane when `/api/config` says `latex: true`, and offers the
   source otherwise. `latex.js`'s `DEFAULT_BASE` of `/latex/` is what a
   deployment serves, so nothing points the module anywhere else.
5. **Renderings.** The `PUT`, the acceptance rules, the quota, the pruning,
   the "rendered from an earlier version" line. A reader who never chose a
   distribution reads a rendering and comments on it.
6. **SyncTeX.** Both directions, on a distribution that returns one.
7. **Optional local command-line compilation.** Validate a local runner
   for the browser compiler artifacts, its installation requirements, and
   platform support. Integrate it with the Rust CLI to compile locally and
   upload renderings under the same rules as the browser. Source-only
   publishing does not depend on this step.

Step 3 gives an author a LaTeX editor with comments and no readers; step 5
gives them readers. Nothing after 5 is required for the project to have
LaTeX.

## Risks

- **Anchoring in a text layer.** pdf.js's text extraction is good and not
  perfect: hyphens at line ends stay hyphens, ligatures may be one
  character or two depending on the font, and a page break is a gap in
  the sequence. Bounded by `viewer-check.mjs`'s miss rate, by the existing
  tolerance of the anchoring, and by the fact that a comment whose anchor
  fails is orphaned rather than lost, as today.
- **A distribution changing under us.** Each is a project with its own
  release cadence, and SwiftLaTeX in particular has been quiet for
  stretches. Bounded by the mirror: a Komodoc deployment fetches what we
  put in the bucket, and an upstream release becomes a new manifest when
  we have tested it, not before.
- **The log parser missing an error.** TeX's log format is not a format.
  Bounded by the raw log being one click away, by the corpus, and by the
  rule that an unrecognised line is not an error rather than a guess.
- **Browser storage.** A distribution of tens of megabytes plus packages
  can exceed what a browser grants without persistence, and Safari evicts
  after a week unused. Bounded by asking for persistence, by saying on the
  card that a refusal means a re-fetch some day, and by the digested URLs
  making a re-fetch cheap to reason about.
- **Cost on the sandbox.** A rendering is a PDF where before there was a
  source. Bounded by `max_document`, by the quota, by pruning to one
  rendering plus labelled ones, and by the sandbox's hourly expiry, which
  applies to renderings as to everything else.
- **Fonts under XeTeX and LuaTeX.** There are no system fonts in a worker.
  A document that asks for one by name gets a fallback or an error, and
  the card says so for those engines.
- **Licences.** The distributions are fetched at runtime and driven through
  glue we write; that is aggregation, not derivation. If a reading of the
  AGPL that disagrees is put to the project, the answer is to keep the
  distributions in their own repositories with their own licences, which
  is where they are.

## Non-goals

- Converting LaTeX to HTML. The PDF is the document.
- A TeX distribution in the binary, or a `make latex`.
- Fetching packages from a distribution's own servers at runtime.
- Choosing a TeX Live version per document. The card offers distributions;
  a distribution is a version.
- A visual editor, a symbol palette, spell-checking. Editor features, and
  not LaTeX ones.
- Compiling on the server. There is no server that compiles anything, and
  this spec keeps it that way.

## Decisions taken here, so they need not be reopened

- The compiler is chosen and downloaded by the reader in the browser, once,
  from a card in the empty preview pane. It is never part of a build.
- Distributions are served from a mirror under the deployment's control,
  through the deployment's own origin, not from upstream; the sandbox's
  bucket is the default mirror.
- Which distributions the card offers is a measurement, recorded as `shown`
  in the manifest, not an opinion in the reader.
- The preview and the document are a PDF, drawn by pdf.js in the existing
  frame, with comments anchored into its text layer. There is no HTML.
- Renderings are stored, as the one exception to "nothing derived is
  stored", keyed by the checkpoint SHA, pruned to the newest plus the
  labelled, counted against the quota, and never required of a reader.
- The log is parsed in JavaScript into `engine/src/diagnostic.rs`'s shape, and
  a line the parser does not recognise is not an error.
- A document is a directory, so `\input`, `\include`, a `.bib` and figures
  beside the main file reach the engine; the glue is ours, one file per
  distribution, behind one interface. Nothing from TeXlyre's editor is
  imported; its worker code is read.
