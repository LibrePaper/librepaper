# SPEC: remaining LaTeX work

## SyncTeX

The reader already has a place-mapping between the source and the page for
Markdown and Typst, `sync.sourcePlaceFor`, built on the text the engine emits.
Typst's PDF text layer uses that same text-based heuristic; it does not require
SyncTeX. For LaTeX's PDF the mapping is SyncTeX's, and the compiler writes it for
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
