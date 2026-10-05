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

The Bun scripts in `web/package.json` and the repository-level `tools/test/suite`
command are the authoritative entry points. Files may also be run directly with
Node when a script documents its prerequisites.

Browser launchers share `web/tools/browser-executable.mjs`. Set
`LIBREPAPER_CHROMIUM` to an executable path to select a specific browser; when
unset, the resolver looks for `chromium`, `chromium-browser`, `google-chrome`,
then `google-chrome-stable` on `PATH`. Running `tools/test/suite browser` or
`tools/test/suite smoke` without a usable browser fails the requested suite rather
than reporting a successful skip. Individual checks that explicitly skip when
their built shell or fixture artifact is absent keep that optional behavior.
The suite pins browser-selecting fixtures to Chromium even when `BROWSER` is
set in the caller's environment. The LaTeX browser fixture can still be run
directly with `firefox`, `chromium`, or `both` for standalone cross-browser
coverage.

## slow-subscriber-recovery

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
