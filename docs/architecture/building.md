---
title: "Building from source"
---

## Building from source

The binary embeds the web build. Browser renderers are fetched separately from the configured asset mirror.

- `web/` (Svelte, Skeleton, CodeMirror 6, Loro) uses bun and vite.
- `crates/` (server, CLI and local service, as internal sub-crates) uses cargo.

Renderers live in `wasm-*` repositories. The browser uses pinned WebAssembly artifacts. The production server binary does not link native document renderers. The web build writes `web/dist`, which the binary embeds.

```sh
make web                         # pages from web/
node tools/assets/pins.mjs fetch # pinned browser renderers and fonts
make build                       # dist/librepaper with embedded pages
make install                     # to ~/.local/bin (override PREFIX= or BINDIR=)
make test                        # web and Rust checks
```

Build needs [bun](https://bun.sh) and Node.js. Rust builds for `x86_64-unknown-linux-gnu` also need `mold`, which `.cargo/config.toml` links through. Pinned inputs are fetched from exact tags and SHA256 digests in `assets.lock`. To move a pin, run `node tools/assets/pins.mjs update release --repo <repo> --tag vX.Y.Z`, then review the lockfile diff.

The browser modules are not embedded. `node tools/assets/pins.mjs fetch` fetches them into `web/wasm/`, which is ignored. They are `markdown`, `org`, `bibliography`, `citations`, `typst`, `typst-syntax`, `pandoc` and `harper`. The editor fonts come from the `editor-fonts` repository into `web/fonts/`. The CodeMirror binding comes from a pinned fork into `web/vendor/loro-codemirror/` (see `web/loro-codemirror.lock`). `tools/assets/mirror publish` uploads the modules to `wasm/<sha256>/<module>` and the fonts to `fonts/<sha256>/<font>.woff2` in the asset mirror. The LaTeX release is at `latex/<sha256>/`, pinned by the same lock. The server gives browsers these URLs from `[assets].mirror` in its TOML, or from the built-in mirror when that is omitted. See [asset mirrors](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md).

### Demo deployment

`make demo` builds and runs `dev` from a worktree in the git directory (`.git/librepaper-demo`), not the current checkout. `DEMO_REF=` picks another branch or commit. It builds the static site and runs the services in `deploy/compose.yaml` and `tools/dev/demo/compose.yaml`, with an app image built from that worktree. It requires Docker Compose 2.24.4 or later. The demo uses the `librepaper-demo` Compose project and loopback ports 8081 (app) and 8082 (site). `PORT=`, `SITE_PORT=` and `DEMO_PROJECT=` change these. Named volumes survive Ctrl-C and `make demo-stop`. `make demo-wipe` deletes them. These targets do not touch `make serve` or `make wipe`.

The demo serves operator graphs at `http://admin.localhost:8081/` with the password `librepaper-demo-only-password`. The first run builds the container image, and later runs reuse its cache. Custom `DEMO_PROJECT` names must start with `librepaper-demo`.

The default configs are `tools/dev/demo/config.toml` and `tools/dev/demo/config-oauth.toml`. GitHub sign-in uses `tools/dev/keys.yaml` when `sops` can decrypt it, or GitHub client credentials from the environment. Otherwise the demo starts without sign-in. That file holds only the development OAuth app, so the demo never loads production secrets. `CONFIG=path/to/config.toml` mounts a config unchanged into the app container. Such a file must use container paths and bind the app to a container address, as the shipped demo configs do (`0.0.0.0:9091`). `MIRROR=` sets the LaTeX mirror the demo checks. It defaults to the pinned release on the public asset mirror. A local directory, such as `../wasm-latex/mirror`, also works. The check is skipped while the mirror and `assets.lock` are unchanged since the last passing check.

`make demo-desktop` builds the desktop app from the same worktree and opens it against the demo. If no demo answers on the demo port, it starts one, and the log goes to `target/demo-desktop.log` in the worktree. Quitting the app stops a demo it started. If a demo is already running, the app uses it and the demo keeps running.

## Workspace

`crates/` holds internal sub-crates and the `librepaper` facade. An edit recompiles the crate it touches and its dependents.

```sh
cargo check -p librepaper-engine           # inner loop: one crate
cargo nextest run -p librepaper-engine     # its tests
make test                                  # everything, before handing work back
```

## Test suites

`make test` runs the web checks and the Rust checks in parallel: svelte-check, unit and integration tests, tool tests, rustfmt, clippy and nextest. It leaves out browser component tests, PostgreSQL-gated cases and deployment scenarios. Run them with:

- `make check`: `make test`, plus `tools/test/suite browser`, `e2e` and `integration`. Those need Chromium and Docker.
- `tools/dev/db test`: the PostgreSQL-gated cases, in a throwaway Docker container. CI runs the same command.
- `tools/test/suite external`: Quarto, R, Python and local-service integrations.
- `tools/test/suite backup`: the backup and restore drill.

For one PostgreSQL-gated case, point `LIBREPAPER_TEST_POSTGRES_URL` at a dedicated, disposable database and select the test by name:

```sh
LIBREPAPER_TEST_POSTGRES_URL='postgresql://postgres:password@127.0.0.1:5432/librepaper_test' \
  cargo test -p librepaper-server --lib server::history_frontier_tests::delegated_agent_bearer_stops_working_after_session_revocation -- --ignored --test-threads=1 --exact
```

The selected case may clear tables, so never point it at a shared or production database. The log and catalogue tests are in `librepaper-engine`. The others are in `librepaper-room` and `librepaper-server`. Change `-p` to match.

Benchmarks are opt-in. Select one ignored test by name, and never run all ignored tests together. The release throughput benchmark uses the destructive benchmark database setting and a bounded override for a short diagnostic:

```sh
LIBREPAPER_BENCHMARK_POSTGRES_URL='postgresql://postgres:password@127.0.0.1:5432/librepaper_bench' \
LIBREPAPER_BENCHMARK_SECONDS=30 \
  cargo test -p librepaper --test storage_benchmarks typing_throughput_release_benchmark --release -- --ignored --nocapture --test-threads=1
```

A short run is for diagnosis only. Release acceptance uses the default 600-second run. The throughput benchmark truncates its database. The socket benchmark uses `LIBREPAPER_BENCH_POSTGRES_URL` instead. The mixed workload is the `frugal_mixed_workload_benchmark` test in `crates/librepaper/tests/frugal_mixed_bench.rs`. Run the relevant benchmark before a performance release, and the recovery drill before a storage or recovery release. The drill's disposable-database checks and prerequisites are in [`tools/test/backup/README.md`](../../tools/test/backup/README.md).

The render tests in `librepaper-companion` are ignored because they need Quarto, Calepin, latexmk or TeX Live installed. `tools/test/suite external` runs all of them. CI runs only `real_quarto_render_reports_success_and_retains_a_failure_log`. A Quarto PDF preview test is kept out of CI because it repeatedly terminated the CI runner. The cause is unconfirmed, so this remains open coverage work. To run the Quarto tests yourself, install Quarto and run:

```sh
cargo test -p librepaper-companion render -- --ignored --test-threads=1
```

### Web tests

- `unit/` runs isolated production modules under Node, without Chromium, a local service or a built compiler artifact.
- `integration/` crosses module boundaries, or uses a real WASM artifact, local service, renderer, compiler adapter or example document.
- `browser/` drives the built Svelte application, or a real page, in Chromium.
- `fixtures/` holds inputs shared by tests. The Typst corpus serves both the PDF and viewer checks.

Entry points are the Bun scripts in `web/package.json` and `tools/test/suite`.

Browser selection:

- Set `LIBREPAPER_CHROMIUM` to an executable path. If it is unset, the resolver looks for `chromium`, `chromium-browser`, `google-chrome` or `google-chrome-stable` on `PATH`.
- `tools/test/suite browser` and `tools/test/suite smoke` fail when no usable browser is found.
- The LaTeX browser fixture can run with `firefox`, `chromium` or `both`.

Prerequisites for `slow-subscriber-recovery`, run with `cd web && bun run check:recovery`:

- Chromium on `PATH`, and installed web dependencies
- a built `target/debug/librepaper`, or `LIBREPAPER_TEST_BINARY`
- `LIBREPAPER_TEST_POSTGRES_URL` pointing to an administrative PostgreSQL database. The role must be able to create and drop databases.
- `LIBREPAPER_TEST_PSQL` and `LIBREPAPER_TEST_PSQL_URL` when `psql` runs in a container

## The interface

Pages use [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton provides buttons, cards, inputs, tables, dialogs, tooltips and toasts. `web/src/styles/theme.css` assigns every colour to a role, from the neutral scale, the brand colour and the status hues. `web/tests/unit/vocabulary.js` enforces three rules, which `make test` runs:

1. Colours and sizes come from the theme, not hex values or arbitrary Tailwind sizes.
2. A control is a component. There is one `IconButton`.
3. Layout comes from `Page`, `Stack` and `Row`.

Agent highlights are drawn on the document origin, not in this stylesheet. Identifying colours in shared sessions are sent over the wire, not chosen locally.
