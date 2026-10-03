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

The Bun scripts in `web/package.json` and the repository-level `tools/suite`
command are the authoritative entry points. The unit and integration lists
below give representative examples, not a complete file inventory. The browser list is complete for
`web/tests/browser/`; files may also be run directly with Node when a script
documents its own prerequisites.

Browser launchers share `web/tools/browser-executable.mjs`. Set
`LIBREPAPER_CHROMIUM` to an executable path to select a specific browser; when
unset, the resolver looks for `chromium`, `chromium-browser`, `google-chrome`,
then `google-chrome-stable` on `PATH`. Running `tools/suite browser` or
`tools/suite smoke` without a usable browser fails the requested suite rather
than reporting a successful skip. Individual checks that explicitly skip when
their built shell or fixture artifact is absent keep that optional behavior.
The suite pins browser-selecting fixtures to Chromium even when `BROWSER` is
set in the caller's environment. The LaTeX browser fixture can still be run
directly with `firefox`, `chromium`, or `both` for standalone cross-browser
coverage.

## Classification

### Unit

`activity`, `agent-client`, `anchor`, `annotating`, `annotation-bar`,
`assistant`, `assistant-preview`, `assistant-review`, `build-catalog`,
`build-preferences`, `citations`, `collab-awareness`, `commands`,
`companion-status`, `deployment-helper`, `diagnostics`, `downloads`, `file-manager`,
`browser-executable`, `browser-suite-selection`, `frame-overlays`, `generation`, `history-calendar`, `insert`, `landing`,
`latex-biber`, `latex-bibliography`, `latex-driver`, `latex-engine`,
`latex-log`, `latex-reader`, `loro-codemirror`, `math`, `offline-projects`,
`orphan`, `outline`, `panels`, `passage-trace`, `passages`, `pdf-fit`,
`preferences`, `presence-colour`, `presence-distinct`,
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
`quarto-local`, `renderer-wasm`, `renderer-worker`,
`slow-subscriber-recovery`, `sync`, and `typst-pdf`.

`slow-subscriber-recovery` also requires Chromium on PATH, installed web
dependencies, a built `target/debug/librepaper` (or `LIBREPAPER_TEST_BINARY`),
and `LIBREPAPER_TEST_POSTGRES_URL` pointing to an administrative PostgreSQL
database. It creates and drops its own database; the role needs permission to
do both. Run it with `cd web && bun run check:recovery`.

If `psql` runs in a container, set `LIBREPAPER_TEST_PSQL` to a command such as
`docker exec -i librepaper-postgres psql` and `LIBREPAPER_TEST_PSQL_URL` to the
administrative database URL as seen inside that container. Missing optional
binary/database configuration skips the check; configured setup failures fail.
The test checks a healthy reader before stalling transport, automatic
reconnection through the production collaboration controller, unsent edits
restored from IndexedDB before reconnection, and a fresh browser after a server
restart. The fixture exercises document-frame dispatch without rendering the
full Reader UI.

### Browser (complete file inventory)

`accessibility-browser`, `agent-browser`, `assistant-review-browser`,
`backups-settings-browser`, `changes-browser`, `citations-browser`,
`comments-bulk-browser`, `comments-paging-browser`, `connections-browser`,
`doc-rows-browser`, `editor-browser`, `files-browser`,
`frame-isolation-browser`, `history-panel-browser`, `history-reader-browser`,
`insert-browser`, `landing-browser`, `latex-browser`, `latex-html-browser`,
`local-consent-browser`, `local-settings-browser`, `math-browser`,
`menubar-browser`, `offline-reload-browser`, `outline-browser`,
`responsive-browser`, `settings-pages-browser`, `settings-save-browser`,
`share-panel-browser`, `shortcuts-browser`, `status-popover-browser`,
`typst-viewer`, `upload-browser`, and `viewer`.
