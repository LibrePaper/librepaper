# Repository tools

`deploy/` is the public Docker kit for self-hosting. `tools/deploy/` operates
the official LibrePaper instance and holds its operator configuration and
private runbook.

`tools/assets/pins.mjs` is the Node CLI for fetching and updating pinned
browser-renderer, CodeMirror, and LaTeX inputs. `tools/assets/mirror` builds,
checks, and publishes the browser asset mirror. LaTeX builds use the adjacent
`wasm-latex` checkout; `MIRROR=...` selects a mirror directory for commands
that consume the built mirror.

`tools/dev/` contains local development helpers, including the persistent
development database. `tools/test/` contains optional test suites, fixtures,
and the backup drill. `tools/test/suite e2e` runs browser deployment scenarios
against disposable Docker PostgreSQL; set `LIBREPAPER_TEST_BINARY` to use an
existing binary instead of building the default. The optional LaTeX deployment
check is `cd web && bun run check:e2e:latex`; it needs a built binary, a
`LIBREPAPER_TEST_POSTGRES_URL`, Chromium, and a LaTeX mirror at
`../wasm-latex/mirror` or a `MIRROR` URL.

`tools/release/` contains release workflow helpers for publishing package
manager manifests. In `web/`, `tools/` holds build-only scripts;
shared browser-test helpers and end-to-end scenarios live under
`tests/helpers/` and `tests/e2e/`.
