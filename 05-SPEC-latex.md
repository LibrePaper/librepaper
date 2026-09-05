# SPEC: LaTeX, compiled in the browser, from a distribution the reader chooses

Status: proposed. Nothing here is built. Extends the Rust host with browser
compiler workers and PDF storage routes. It is written against the
editor `01-SPEC-history.md` describes, where readers render the text
themselves, and it makes one exception to that spec's "nothing derived is
stored", stated and bounded below. It reuses the diagnostic shape
of `02-SPEC-diagnostics.md` and the preview frame of the reader as it stands.

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
ports of pdfTeX and XeTeX, and BusyTeX, which packs TeX Live 2026 with
pdfTeX, XeTeX and LuaTeX into one module -- and for typst it uses typst.ts.
It is AGPL-3.0, funded by NLnet's NGI0 Commons Fund under the European
Commission's Next Generation Internet programme, and has roughly nine
hundred GitHub stars. The compilers exist, they run in a worker, and a
document of ordinary size compiles in seconds. What Komodoc has to decide is
not whether a browser can compile LaTeX but where the compiler comes from,
what it produces, and how a reader who never compiles anything sees the
result.

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
distributions Komodoc knows how to drive, each with its engines, its
download size and what it can and cannot do. Nothing is fetched until one
is chosen. The chosen distribution is stored in that browser and is not
asked for again, for that document or any other; the card is reachable
afterwards from the preview toolbar to switch. A Komodoc build carries no
TeX and there is no `make latex`; a server serves the distributions as
static objects, or points at a bucket that does.

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
`02-SPEC-diagnostics.md` defines, so the underline and the gutter mark are the
same ones typst errors get.

Markdown and typst documents are untouched by everything below. The format
is `latex`, the file extension `.tex`, and every surface here is inert for
any other format.

## The distributions

A distribution is a set of static files a browser fetches and runs: one or
more WebAssembly modules, the JavaScript Emscripten generated to host them,
and the TeX Live files the engine reads -- formats, `.sty`, `.cls`, `.tfm`,
fonts, hyphenation patterns. Komodoc drives them through one interface,
described under "The worker", and knows these:

| distribution | engines | bibliography | what is fetched |
| --- | --- | --- | --- |
| SwiftLaTeX pdfTeX | pdfTeX | in step 1 | a small module up front; every package file on first use |
| SwiftLaTeX XeTeX | XeTeX, then dvipdfmx | in step 1 | as above; system fonts are not available, only fetched ones |
| BusyTeX, TeX Live 2026 | pdfTeX, XeTeX, LuaTeX, BibTeX | BibTeX | one large module and a package bundle up front; further bundles on demand |

The sizes are deliberately not written here. They are measured in step 1
and written on the card, because the card is the only place the number
matters and the one place it must be right. The shape of the trade is
known: SwiftLaTeX is small to start and chatty afterwards -- a compile that
meets a new package stops, fetches it, and resumes -- and BusyTeX is large
to start and quiet afterwards. The card says which is which in those words.

Every file a distribution fetches is served from one base URL, `--latex
<url>` on the server, which defaults to a public bucket the project keeps
for the sandbox and that a self-hoster may use, mirror, or replace. The
browser never fetches from a distribution's own project site at runtime.
Two reasons. A fetched package is a package name sent to whoever serves it,
and the list of packages a document uses is a description of the document;
that list goes to the deployment the author already trusts with the source,
and nowhere else. And a third-party endpoint that goes away, or changes its
layout, would take every LaTeX document on every Komodoc deployment with
it. A mirror under our own name is a directory of files with a manifest; it
changes when we change it.

Fetched files are kept in the browser's Cache Storage under the
distribution's base URL, so a package is fetched once per browser, not once
per document or once per compile. A distribution's manifest names every
file with a digest, and the file URLs carry the digest, as `typst.wasm`'s
does today, so cached forever is safe and an updated distribution is a new
manifest rather than a cache to invalidate. Storage is asked for
persistence once, when a distribution is chosen; if the browser refuses,
the distribution still works and may need to be fetched again some day,
which the card says.

The shell's CSP already permits this: `default-src` and `script-src` allow
`https:` and `blob:`, and a worker's source falls back to `script-src`. A
deployment whose `--latex` points at an `http:` mirror on a private network
gets a clear refusal at startup rather than a silent CSP failure in every
browser.

## The worker

One module, `web/src/lib/latex.js`, plays the part `renderers.js` plays for
the engine: it owns the distribution, the worker, and the promise of the
next compile. It presents one interface to the reader and hides three
different Emscripten programs behind it.

```js
choose(name)                      // fetch, cache and load a distribution; a promise
chosen()                          // the name stored in this browser, or null
compile(source, name) -> { pdf, synctex, log, diagnostics, seconds }
```

`compile` writes the source into the engine's in-memory filesystem as
`<name>.tex`, runs the engine with SyncTeX on, runs BibTeX and the second
and third passes when the log asks for them, and returns the PDF bytes, the
SyncTeX file, the raw log, the diagnostics parsed from it, and how long it
took. It runs in a Web Worker because a compile holds the thread for
seconds and the editor must not freeze for it. A compile requested while
one is running waits, and a request that arrives while another is waiting
replaces it: at most one compile runs and one is queued, and the queued one
is always the latest text.

The distribution-specific glue is one file each, `latex/swiftlatex.js` and
`latex/busytex.js`, written by us against each distribution's exported
API. Nothing of TeXlyre's editor is imported. Its worker code is the best
available reading of how each engine wants to be driven and is read for
that; what is written here is ours, because the interface above is not
theirs and because the licences differ: TeXlyre and SwiftLaTeX are AGPL-3.0,
BusyTeX is MIT, and the distributions are fetched at runtime as separate
works rather than linked into Komodoc's own build. The licence of each
distribution is named on the card.

The log is parsed in JavaScript, not in the engine crate, because the
compiler is not our crate and there is nothing on the Rust side to parse
it. A `! message` line followed within a few lines by `l.<n>` is an error
at line `n` of the current file, which the `(` and `)` file-tracking in the
log names; the message is the `!` line, the hints are the lines between it
and `l.<n>`, and the span is the whole line, because TeX reports no column.
`LaTeX Warning:`, `Overfull \hbox`, `Underfull \hbox` and `Package <p>
Warning:` are warnings with the line number when the log gives one and
`line` 0 when it does not. The parser is a pure function of the log string
with a table-driven test suite of logs collected in step 1, and it is
expected to miss things: a log line it does not recognise is not an error,
and the raw log is one click from the badge for whatever the parser did
not catch. The diagnostic shape is `02-SPEC-diagnostics.md`'s, unchanged, with
`file` empty for the document and the included file's name otherwise.

A rendering that references files the document does not have -- an
`\input`, an `\includegraphics`, a `.bib` -- fails with the error TeX
gives, and that error is shown where it is. A document is one text, here
as everywhere in Komodoc; `filecontents*` is the supported way to carry a
bibliography inside it, and the card says so in one sentence. Files beside
the document are the same future that `04-SPEC-sync.md` names out of scope
for the session, and this spec does not open it.

## The preview

The preview pane in a LaTeX document has three states, and the frame is the
same frame with the same agent in each.

**No distribution.** The card. It lists the distributions with engine,
licence, size and the one-line trade, with the one this browser used before
preselected if any. It also says what "chosen" means: a download of that
size, once, kept in this browser. Choosing starts the fetch with a progress
bar in the pane; the editor is usable throughout. For a reader who may not
edit, this state does not occur: they see "not yet rendered" and the source
if they want it, because a reader is never asked to download a compiler.

**Compiling.** The last PDF that compiled stays up, dimmed by nothing; the
badge says "compiling" with a spinner and, after the first compile, the
seconds the last one took. On failure the badge says how many errors, as
for typst, and the last page stays. A compile starts after the source has
been quiet for `1500` milliseconds -- twenty-five times the typst delay and
still well under the compile itself -- and the delay is a constant in
`latex.js`, not a setting, because a setting nobody changes is a lie in the
docs.

**Rendered.** The PDF in the frame. The frame's document is a small viewer
page Komodoc serves on the documents origin: pdf.js from our own static
assets, the agent as always, and a `preview` message that carries PDF bytes
instead of HTML. The viewer draws every page's canvas and text layer;
pages are stacked vertically in one scrolling document, which is what
makes "the text of the document" one sequence for anchoring. The agent's
walk over text nodes is unchanged; its table is rebuilt when pages render,
which the viewer signals the same way an HTML document's mutation observer
does. Highlights are painted as they are in HTML: the text layer's spans
are ordinary elements, and a mark across them is a mark.

Text-quote anchoring across a page break, a hyphenated line end, or a
ligature that pdf.js extracts as one glyph is the risk here and is listed
below. Its mitigation is that the anchoring is already tolerant of
whitespace and already re-anchors with context; a LaTeX document is a hard
case for the same anchoring, not a new anchoring.

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
`max_html`, which is renamed `max_document` in the same change because it
now bounds a PDF as readily as an HTML file. A rendering counts against its
owner's quota. It is derived, so unlike a checkpoint it may be removed: a
rendering is kept for the newest checkpoint that has one and for every
labelled checkpoint, and the server prunes the rest on the retention
schedule. That keeps the cost per document at one PDF plus one per name
the author gave, and on the sandbox, whose cost is the first constraint of
every spec here, a PDF of a few hundred kilobytes for an hour is what a
checkpoint of the source already is, ten times over.

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
stores the source with format `latex` and, in the first version, renders
nothing: no TeX ships in the executable, and this spec does not put one
there. Optional local CLI compilation is a later step. First choose and
test a local runner for the same browser compiler artifacts: the Rust
executable does not supply the JavaScript environment the Emscripten glue
needs. A separately installed runner may be required for that optional
command, but never for `serve`, source-only publishing, or reading. The
distribution cache belongs on the client machine, and no compilation is
moved to the deployment.

## SyncTeX

The reader already has a place-mapping between the source and the page for
markdown and typst, `sync.sourcePlaceFor`, built on the text the engine
emits. For a PDF the mapping is SyncTeX's, and the compiler writes it for
free. The `.synctex.gz` is parsed in the viewer into two tables, source
line to page and box, and page position to source line, and the two
existing gestures are wired to them: the caret's line scrolls the frame to
its box and outlines it for a moment, and a double-click on the page moves
the editor's caret to the line. Neither is needed for reading and
commenting, which is why they are the last step and not the first; both
are what the research note calls table stakes for a LaTeX editor, and both
are a parse of a file the compiler already produces.

## Steps

1. **The distributions, measured.** Mirror each distribution to the bucket
   with a manifest; measure the up-front and typical-document download for
   each; collect a corpus of logs from the `examples/` LaTeX documents
   written for this step, compiled by each engine, for the parser's tests.
   Decide from the measurements whether all three distributions stay on the
   card or whether one is enough at first.
2. **The worker.** `latex.js` and the two glue files; `choose`, `chosen`,
   `compile`; Cache Storage with digested URLs; the log parser and its
   tests. Tested in a headless browser against the mirror, with the corpus
   compiling to the same page count on each engine.
3. **The card and the compiling state.** The preview pane's three states,
   the debounce, the badge, the last-page-stays rule. A `.tex` document is
   accepted by the server with format `latex`, `source_formats` gains
   `latex`, and the reader opens it in the editor.
4. **The viewer.** The pdf.js page on the documents origin, the `preview`
   message with bytes, the agent's table rebuilt on page render, highlights
   painted in the text layer. The anchoring tests run against a PDF with a
   hyphenated line end, a page break inside a sentence, a ligature, and a
   footnote.
5. **Renderings.** The `PUT`, the acceptance rules, the quota, the pruning,
   the "rendered from an earlier version" line, `max_html` to
   `max_document`. A reader who never chose a distribution reads a
   rendering and comments on it.
6. **SyncTeX.** Both directions.
7. **Optional local command-line compilation.** Validate a local runner
   for the browser compiler artifacts, its installation requirements, and
   platform support. Integrate it with the Rust CLI to compile locally and
   upload renderings under the same rules as the browser. Source-only
   publishing does not depend on this step.

Steps 1 through 4 give an author a LaTeX editor with comments and no
readers; step 5 gives them readers. Nothing after 5 is required for the
project to have LaTeX.

## Risks

- **Anchoring in a text layer.** pdf.js's text extraction is good and not
  perfect: hyphens at line ends stay hyphens, ligatures may be one
  character or two depending on the font, and a page break is a gap in
  the sequence. Bounded by step 4's tests, by the existing tolerance of the
  anchoring, and by the fact that a comment whose anchor fails is orphaned
  rather than lost, as today.
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
- Files beside the document: `\input`, images, a separate `.bib`. That is
  the assets question, and it is not opened here.
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
  not from upstream, and the sandbox's bucket is the default mirror.
- The preview and the document are a PDF, drawn by pdf.js in the existing
  frame, with comments anchored into its text layer. There is no HTML.
- Renderings are stored, as the one exception to "nothing derived is
  stored", keyed by the checkpoint SHA, pruned to the newest plus the
  labelled, counted against the quota, and never required of a reader.
- The log is parsed in JavaScript into `02-SPEC-diagnostics.md`'s shape, and
  a line the parser does not recognise is not an error.
- A document is one file. `filecontents*` is how a bibliography rides
  along.
- The glue is ours, one file per distribution, behind one interface.
  Nothing from TeXlyre's editor is imported; its worker code is read.
- `max_html` becomes `max_document`, in step 5, because it bounds a PDF now.
