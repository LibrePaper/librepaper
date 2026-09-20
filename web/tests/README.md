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

`activity`, `agent-client`, `anchor`, `annotating`, `annotation-bar`,
`assistant`, `assistant-preview`, `assistant-review`, `build-catalog`,
`build-preferences`, `citations`, `collab-awareness`, `commands`,
`companion-status`, `diagnostics`, `downloads`, `file-manager`,
`frame-overlays`, `generation`, `history-calendar`, `insert`, `landing`,
`latex-biber`, `latex-bibliography`, `latex-driver`, `latex-engine`,
`latex-log`, `latex-reader`, `loro-codemirror`, `math`, `offline-projects`,
`orphan`, `outline`, `panels`, `passage-trace`, `passages`, `pdf-fit`,
`playground`, `preferences`, `presence-colour`, `presence-distinct`,
`preview-cache`, `project-session`, `project-upload`, `projection`,
`projection-digest`, `proposal-contention`, `proposal-draft-marks`,
`proposal-marks`, `proposals`, `quarto`, `quarto-options`,
`quota-preferences`, `reader-annotations`, `reader-assistant`,
`reader-preview`, `reader-races`, `reader-source-events`,
`render-coordinator`, `render-diagnostics`, `review-fixes`, `room`,
`settings-account`, `settings-quarto`, `steady-busy`, `submissions`,
`suggestions`, `synctex`, `thread`, `timeline`, `vocabulary`, `workspace`,
and `zip`.

### Integration

`bibliography-wasm`, `companion-lifecycle`, `history-source`,
`insert-latex-render`, `insert-markdown-render`, `insert-quarto-render`,
`insert-typst-render`, `latex-controller`, `latex-html`, `latex-local`,
`latex-resources`, `local-companion`, `local-preview`, `needs`,
`quarto-local`, `renderer-wasm`, `renderer-worker`, `sync`, and `typst-pdf`.

### Browser

`agent-browser`, `assistant-review-browser`, `citations-browser`,
`comments-bulk-browser`, `editor-browser`, `files-browser`,
`history-panel-browser`, `history-reader-browser`,
`insert-browser`, `latex-browser`, `latex-html-browser`,
`local-consent-browser`, `markdown-tracking-browser`, `math-browser`,
`outline-browser`, `responsive-browser`, `shortcuts-browser`, `typst-viewer`,
and `viewer`.
