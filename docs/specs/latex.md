# SPEC: LaTeX, compiled in the browser, from a distribution the reader chooses

Status: the compiler, the mirror, the viewer, the card and stored renderings
are built and are no longer described here. `latex/` builds and serves the
mirror, `web/src/lib/latex.js` and `web/src/lib/latex/` drive four
distributions from a worker and read their logs, `web/src/entries/viewer.js`
and `web/src/lib/pdf/` draw a PDF with comments anchored into its text layer,
`crates/komodoc/src/latex.rs` is `--latex`, `web/src/components/LatexCard.svelte`
is the card, and the renderings routes in `crates/komodoc/src/server.rs` with
their storage and pruning in `crates/komodoc/src/room.rs` are what readers
see, with `crates/komodoc/src/tests/renderings.rs`. The reader stores a
rendering after the quiet minute or on naming a checkpoint, and shows everyone
else the newest one there is. What remains is SyncTeX and the optional local
command. Neither is required for the project to have LaTeX: an author has an
editor with comments, and readers have renderings, without them.

## What the remaining work builds on

| where | what |
| --- | --- |
| `latex/tools/distributions.mjs`, `latex/tools/mirror.mjs`, `latex/tools/serve.mjs` | the four distributions Komodoc drives, as data; the mirror, with digested file names and a `manifest.json` carrying each distribution's engines, licence, measured bytes and `shown` flag |
| `komodoc serve --latex <url-or-dir>` | where the mirror is. Defaults to the project's bucket, refuses plain `http:` at startup, and is served to browsers from `/latex/` on the deployment's own origin, so the list of packages a document asks for goes no further than the deployment that has the source. `/api/config` says `latex: true` when a mirror is configured |
| `web/src/lib/latex.js` | `at(url)`, `available()`, `chosen()`, `choose(name)`, `compile(tree)` returning `{pdf, synctex, log, diagnostics}`. Cache Storage under the mirror's URL, persistence asked for once; at most one compile running and one queued, the queued one always the latest |
| `web/src/lib/latex/worker.js`, `swiftlatex.js`, `busytex.js`, `texlyre.js`, `log.js` | the worker, the glue, one file per distribution, and the log parser, which emits `crates/engine/src/diagnostic.rs`'s shape and treats a line it does not recognise as nothing |
| `web/src/entries/viewer.js`, `web/src/lib/pdf/` | the frame for a PDF, on the documents origin, taking the editor's `preview` message with `pdf` bytes where an HTML document has `html`; every page's canvas and text layer stacked in one scrolling document, the agent's table rebuilt as pages render, highlights painted in the text layer; `komodocViewer.pageForOffset` exposed for SyncTeX |
| `renderings/<slug>/<sha>` and `<sha>.synctex` | a rendering, the PDF an editor's browser compiled, keyed by the checkpoint it was compiled from; the SyncTeX file gzipped beside it when the compiler returned one. `PUT` by whoever may edit, readable by whoever may read, counted against the quota, pruned to the newest plus every labelled checkpoint |
| `latex/corpus/` | the corpus, its logs per engine, `MEASUREMENTS.md`, and `pages.json` from a TeX Live on a desk |
| `web/checks/latex-log.mjs`, `latex-check.mjs`, `viewer-check.mjs` | the parser over every log in the corpus; the distributions in headless Chromium against the mirror; the anchoring miss rate over a hyphenated line end, a page break inside a sentence, a footnote and a ligature |

Of the four distributions only SwiftLaTeX pdfTeX is `shown`. The
measurements found that SwiftLaTeX XeTeX's dvipdfmx has no font to embed and
that BusyTeX cannot load `fontenc`; TeXlyre's TeX Live 2026 BusyTeX is
measured and not yet offered. The flag travels in the manifest, so offering
another is an edit to `distributions.mjs` and a rebuild of the mirror, with no
build of Komodoc involved. The card lists what the manifest marks shown and
names none itself.

A `.tex` file publishes with format `latex`, `source_formats` has `latex`,
and the server compiles nothing: no TeX ships in the executable, and there is
no `make latex`.

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

## The optional local command

The command line stores the source with format `latex` and renders nothing.
Optional local compilation is a later step. First choose and test a local
runner for the same browser compiler artifacts: the Rust executable does not
supply the JavaScript environment the Emscripten glue needs. A separately
installed runner may be required for that optional command, but never for
`serve`, source-only publishing, or reading. The distribution cache belongs
on the client machine, and no compilation is moved to the deployment.

## Steps

6. **SyncTeX.** Both directions, on a distribution that returns one.
7. **Optional local command-line compilation.** Validate a local runner
   for the browser compiler artifacts, its installation requirements, and
   platform support. Integrate it with the Rust CLI to compile locally and
   upload renderings under the same rules as the browser. Source-only
   publishing does not depend on this step.

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
- Renderings are stored, as the one exception to `docs/specs/history.md`'s
  "nothing derived is stored", keyed by the checkpoint SHA, pruned to the
  newest plus the labelled, counted against the quota, and never required
  of a reader.
- The log is parsed in JavaScript into `crates/engine/src/diagnostic.rs`'s shape, and
  a line the parser does not recognise is not an error.
- A document is a directory, so `\input`, `\include`, a `.bib` and figures
  beside the main file reach the engine; the glue is ours, one file per
  distribution, behind one interface. Nothing from TeXlyre's editor is
  imported; its worker code is read.
