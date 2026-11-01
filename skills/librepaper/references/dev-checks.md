# Building and testing

The repo's `AGENTS.md` comes first. It says which checks a change needs, that every change happens in a git worktree, and that subagents never run checks. The main agent runs them once, on the combined result.

## Commands

```sh
cargo check -p <crate>          # while editing; packages are librepaper, librepaper-engine, -server, -room, -document, -base, -shell, -companion
cargo nextest run -p <crate>
make test                       # web checks, rustfmt, clippy, node tool tests, nextest; not everything
tools/test/suite browser        # browser components in real Chromium
tools/dev/db test               # PostgreSQL-gated tests in a throwaway container, as CI runs them
make check                      # make test, then the browser suite and the reader smoke test
make demo                       # try dev as a user: Docker kit built from dev, app on :8081
make serve                      # bare binary with no sign-in flow; only for quick checks that need no publishing
make demo-desktop               # build the desktop app and open it against the running demo
```

`make demo` reads its GitHub sign-in keys from `tools/dev/keys.yaml`, which is encrypted for the maintainers. Without those keys, use `make serve`.

A green `make test` does not mean everything passes. The browser suite and the PostgreSQL-gated tests sit outside it, and both have hidden real faults before.

## Traps

- **New enum variants.** `cargo check -p` on one crate misses matches in downstream crates. Run workspace clippy (`make test`) before trusting a new public variant.
- **SQL changes.** After changing a query or a migration, run `tools/dev/db sqlx-prepare`. `tools/dev/db sqlx-check` shows whether `crates/librepaper-engine/.sqlx/` is stale.
- **Fresh worktrees.** Web checks need `web/node_modules`, `web/vendor` and `web/wasm`, which are gitignored. Symlink them from the main checkout and delete the symlinks before committing. A `git add -A` once committed them, and the merge wiped the main checkout's copies.
- **No `web/dist` in a fresh worktree.** `librepaper-shell` embeds it, so any build of the server there (`tools/dev/db test` included) fails with "proc macro panicked". Symlink `web/dist` from the main checkout for Rust-only work and delete it before committing.
- **Stale `web/dist`.** Browser tests read `web/dist`. Rebuild it fully with `cd web && bun run build` before believing a browser failure. A partial `vite build` drops files and fakes regressions.
- **Browser harness without CSS.** A harness that builds with Vite `lib` mode loads no component CSS unless it sets `cssFileName` and links the sheet. Layout assertions there test nothing. Copy the `<link rel="stylesheet" href="/check.css">` setup from `doc-rows-browser.mjs`.
- **Flaky browser tests.** Rerun a single browser failure before believing it.
- **A worker's new unit file.** Run it alone under `timeout 20 node tests/unit/<file>.mjs` before the suite. A fake-clock test of a scheduler that loops pins a CPU forever, and inside `npm run check` it looks like a hang, not a failure.
- **No nextest.** Without `cargo-nextest`, `make test` falls back to `cargo test`, which stops at the first failing crate. In one shared process the `librepaper-base` sign-in tests collide and fail with "authentication service is busy", and `librepaper-companion`'s `a_torrential_tool_is_bounded` can fail too. Run `cargo test --workspace --no-fail-fast`, then rerun each failure alone before believing it. If rustup lacks rustfmt or clippy, `make test` stops at `cargo fmt --check`; ask the user to run `rustup component add rustfmt clippy`.
- **One PostgreSQL-gated test.** Source `tools/test/postgres.sh`, call `postgres_start_disposable`, set `LIBREPAPER_TEST_POSTGRES_URL` to its port, and run `cargo test -p <crate> --lib <name> -- --ignored --test-threads=1`. This is much faster than all of `tools/dev/db test`. The helper is bash: sourced from the agent's zsh it fails with "did not become ready on 127.0.0.1:" and an empty port. Run it under `bash -c`, or start the container by hand with `docker run -d --name <n> -e POSTGRES_PASSWORD=test -p 127.0.0.1::5432 postgres:17.11-alpine` and read the port from `docker port <n> 5432/tcp`.
- **A worktree's `web/dist` symlink.** Before `bun run build` in a worktree, `unlink web/dist` first. Building through the symlink overwrites the main checkout's dist.
- **Stale embedded shell.** The binary embeds `web/dist` in `crates/librepaper-shell`. If a rebuilt binary still serves the old page, touch `crates/librepaper-shell/src/lib.rs` and build again.
- **The desktop crate is outside the gate.** `desktop/` is its own Cargo workspace; `make test` never compiles it. Test it with `cargo test --manifest-path desktop/Cargo.toml` (and `cargo clippy --manifest-path desktop/Cargo.toml --all-targets -- -D warnings`) inside `nix-shell -p pkg-config webkitgtk_4_1 gtk3 libsoup_3 glib-networking`. Tauri's build script wants the sidecar first: copy `desktop/binaries/librepaper-<triple>` from the main checkout into the worktree. The dependency build is cold per worktree (a few minutes); the crate itself rebuilds in about two.
- **`CARGO_TARGET_DIR` breaks `make check` at the smoke step.** The `dist/librepaper` rule copies from a hard-coded `target/<profile>/librepaper`, so with the variable set it stops with `cannot stat target/release/librepaper` after the browser suite. Everything before it has run; finish with `LIBREPAPER_TEST_BINARY=<that target>/release/librepaper tools/test/suite smoke`. A first smoke failure on "the visible PDF format menu item" is the menu placement race; rerun once.
- **Screenshotting one component.** Lib-build it as `web/tests/browser/changes-browser.mjs` does, write an `index.html` next to the build, serve it with `python3 -m http.server`, and run `chromium --headless=new --window-size=300,640 --virtual-time-budget=3000 --screenshot=out.png <url>`. Python serves CSS without a charset, so a non-ASCII CSS `content:` glyph shows as mojibake there and nowhere else.
