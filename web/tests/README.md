# Web test layout

The checks are grouped by how they execute, not by product feature:

- `unit/` runs isolated production modules under Node. These checks do not
  need Chromium, a local service, or a built compiler artifact.
- `integration/` crosses module boundaries or exercises a real WASM artifact,
  local companion, renderer, compiler adapter, or example document.
- `browser/` drives the built Svelte application or a real browser page with
  Chromium.
- `fixtures/` contains checked-in inputs shared by tests. The Typst corpus is
  used by both the PDF and viewer checks.

The npm scripts in `web/package.json` are the authoritative entry points; the
directory names make the runtime requirements visible without changing what
each check asserts.

## Classification

### Unit

`agent-client`, `anchor`, `assistant-preview`, `assistant-review`, `assistant`,
`citations`, `diagnostics`, `dictation-assemble`, `dictation-capture`,
`dictation-language`, `dictation-models`, `dictation-purge`,
`dictation-segmenter`, `dictation-webspeech`, `diff-display`, `downloads`,
`file-manager`, `insert`, `landing`, `latex-biber`, `latex-bibliography`,
`latex-driver`, `latex-engine`, `latex-log`, `latex-reader`, `latex-route`,
`math`, `orphan`, `outline`, `passages`, `pdf-fit`, `provenance`,
`quarto-options`, `quarto`, `quota-preferences`, `reader-annotations`,
`reader-assistant`, `reader-preview`, `reader-races`, `reader-source-events`,
`redlines`, `review-fixes`, `semantic-redlines`, `settings-quarto`,
`submissions`, `suggestions`, `timeline`, `tree-digest`, and `vocabulary`.

### Integration

`bibliography-wasm`, `companion-lifecycle`, `dictation-service`,
`history-controller`, `insert-latex-render`, `insert-markdown-render`,
`insert-quarto-render`, `insert-typst-render`, `latex-controller`,
`latex-html`, `latex-local`, `latex-resources`, `local-companion`,
`local-preview`, `needs`, `quarto-local`, `renderer-wasm`, `renderer-worker`,
`sync`, and `typst-pdf`.

### Browser

`agent-browser`, `assistant-review-browser`, `citations-browser`,
`comments-bulk-browser`, `editor-browser`, `files-browser`,
`history-merge-browser`, `history-panel-browser`, `history-semantic-browser`,
`insert-browser`, `latex-browser`, `latex-html-browser`,
`local-consent-browser`, `markdown-tracking-browser`, `math-browser`,
`outline-browser`, `responsive-browser`, `typst-viewer`, and `viewer`.
