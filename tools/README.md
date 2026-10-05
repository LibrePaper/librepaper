# Repository tools

`deploy/` is the public Docker kit for self-hosting. `tools/deploy/` operates
the official LibrePaper instance and holds its operator configuration and
private runbook.

`tools/assets/pins.mjs` is the Node CLI for fetching and updating pinned
browser-renderer, CodeMirror, and LaTeX inputs. `tools/assets/mirror` builds,
checks, and publishes the browser asset mirror; its LaTeX build needs the
`wasm-latex` checkout next to this repository, or `MIRROR=...` to select a
mirror directory.

`tools/dev/` contains local development helpers, including the persistent
development database. `tools/test/` contains optional test suites, fixtures,
and the backup drill. `tools/test/suite e2e` runs browser deployment scenarios
against disposable Docker PostgreSQL; set `LIBREPAPER_TEST_BINARY` to use an
existing binary instead of building the default. The optional LaTeX browser
test is `cd web && npm run check:latex-browser`; it needs a LaTeX mirror at
`../wasm-latex/mirror` or a `MIRROR` URL, plus Firefox and Chromium.

`tools/release/` contains release workflow helpers for publishing package
manager manifests. In `web/`, `tools/` holds build and asset-fetch scripts;
shared browser-test helpers and end-to-end scenarios live under
`tests/helpers/` and `tests/e2e/`.
