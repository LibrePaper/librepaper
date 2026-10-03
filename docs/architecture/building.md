---
title: "Building from source"
---

## Building from source

The binary embeds the web build. Browser renderers are fetched separately from the configured asset mirror.

- `web/` (Svelte, Skeleton, CodeMirror 6, Loro): bun and vite
- `crates/librepaper/` (server and CLI): cargo

Renderer implementations live in `wasm-*` repositories. The browser uses pinned WebAssembly artifacts; the production server binary does not link native document renderers. Some renderer crates remain dev-dependencies for fixtures and tests. The web build writes to `web/dist`, which the binary embeds.

```sh
make web                # pages from web/
tools/pins fetch        # pinned browser renderers
make build              # dist/librepaper with embedded pages
make install            # to ~/.local/bin (override PREFIX= or BINDIR=)
make test               # rustfmt, clippy and test suite
tools/suite external    # Quarto/R/Python and local-service integrations
```

Build needs [bun](https://bun.sh) and Node.js. Rust builds for `x86_64-unknown-linux-gnu` also require `mold`, configured in `.cargo/config.toml`. Browser renderers are fetched from exact tags and SHA256 digests in `assets.lock`. To update a renderer: `tools/pins update wasm wasm-markdown v0.2.0`, then review the lockfile diff.

The four browser modules (markdown, bibliography, citations, typst) are not embedded. `tools/pins fetch` fetches them to `web/wasm/` (ignored). `tools/deploy-assets publish` publishes to the asset mirror at `wasm/<sha256>/<module>` (SHA256 from `assets.lock`). The same lock pins LaTeX at `latex/<sha256>/`. The server passes these URLs to browsers on the mirror named by `--asset-mirror`. See [asset mirrors](https://github.com/LibrePaper/librepaper/blob/main/docs/dev/asset-mirrors.md).

`make demo` runs the site, application, local companion and simulated activity. It runs `tools/deploy-assets check` on the LaTeX mirror at `MIRROR=` (default `../wasm-latex/mirror`), then Docker PostgreSQL from `tools/db dev` or `LIBREPAPER_DATABASE_URL`. GitHub sign-in comes from `tools/deploy-keys.yaml` when `sops` can decrypt it; otherwise none. Use `LIBREPAPER_PUBLISHERS=any LIBREPAPER_COMMENTERS=anyone` for local development.

## Workspace

`crates/` holds internal sub-crates and the `librepaper` facade. An edit recompiles the crate it touches and its dependents.

```sh
cargo check -p librepaper-engine           # inner loop: one crate
cargo nextest run -p librepaper-engine     # its tests
make test                                  # everything, before handing work back
```

- The published `librepaper` crate is generated, not checked in: `tools/flatten/flatten build` writes `target/flat/`.
- `tools/flatten/flatten lint` checks the rules the generator relies on; CI runs both, then tests the flat crate.
- `tools/flatten/build.rs` is the flat crate's build script. Every sub-crate `build.rs` needs its counterpart there.

## Test suites

`make test` runs the ordinary Rust and web checks; PostgreSQL-gated cases and
benchmarks remain ignored so a local run never assumes it may alter a database.
For a focused PostgreSQL case, point `LIBREPAPER_TEST_POSTGRES_URL` at a
dedicated disposable database and select one test by name:

```sh
LIBREPAPER_TEST_POSTGRES_URL='postgresql://postgres:password@127.0.0.1:5432/librepaper_test' \
  cargo test -p librepaper --lib server::history_frontier_tests::delegated_agent_bearer_stops_working_after_session_revocation -- --ignored --test-threads=1 --exact
```

The log and catalogue tests live in `librepaper-engine`, the room and server tests in `librepaper`: run the same command with `-p librepaper-engine` for the former.

The selected case may clear tables, so do not use a shared or production
database. Shared PostgreSQL fixtures fail when explicitly run without their
required database setting. The PostgreSQL CI job runs its recovery and deployment gates serially
against separate disposable databases; its explicit selection skips keep
release/capacity benchmarks and the unstable Quarto preview out of that run.
See the [PostgreSQL CI invocation](../../.github/workflows/ci.yml).

Keep benchmarks opt-in by selecting one ignored test by its function name;
do not run all ignored library tests together. For example, the release
throughput measurement uses the destructive benchmark database setting and a
bounded override when a short diagnostic is intended:

```sh
LIBREPAPER_BENCHMARK_POSTGRES_URL='postgresql://postgres:password@127.0.0.1:5432/librepaper_bench' \
LIBREPAPER_BENCHMARK_SECONDS=30 \
  cargo test -p librepaper --lib typing_throughput_release_benchmark --release -- --ignored --nocapture --test-threads=1
```

The short run is diagnostic only; use the default 600-second run for release
acceptance. Run the relevant benchmark before a performance release, and run
the isolated recovery drill before a storage or recovery release. The throughput
benchmark truncates its configured database. The socket benchmark instead
uses `LIBREPAPER_BENCH_POSTGRES_URL`; the mixed workload and its PostgreSQL
setup are documented in [`tools/frugal-mixed-bench/README.md`](../../tools/frugal-mixed-bench/README.md).
The isolated backup/restore recovery drill has its own disposable-database
checks and exact prerequisites in [`tools/frugal-recovery/README.md`](../../tools/frugal-recovery/README.md).

The real Quarto PDF preview test is still unfinished CI coverage. Its current
symptom is repeated CI runner termination; the cause is unconfirmed. To
reproduce it, install Quarto and a PDF engine, then run the focused ignored
test:

```sh
cargo test -p librepaper --lib real_quarto_pdf_preview_publishes_complete_pdf_bytes -- --ignored --test-threads=1
```

`tools/suite external` is the current manual coverage path for Quarto/R/Python
and local-service integrations. The preview can return to CI after the
termination cause is isolated and the test has a bounded, reliable CI run.

## The interface

Pages use [Skeleton](https://skeleton.dev) on Tailwind 4. Skeleton provides buttons, cards, inputs, tables, dialogs, tooltips and toasts. `web/src/styles/theme.css` colours everything from four colours (palette written once). Three rules, enforced by `make test`:

1. Colours and sizes come from theme, not hex values or arbitrary Tailwind sizes.
2. A control is a component; there is one `IconButton`.
3. Layout comes from `Page`, `Stack` and `Row`, assembled not measured.

Agent highlights are drawn on the document origin (outside this stylesheet). Identifying colours in shared sessions travel over the wire (not a local decision).
