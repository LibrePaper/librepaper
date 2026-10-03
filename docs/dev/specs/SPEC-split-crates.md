# Proposed: Split the single crate into workspace sub-crates

**Status:** Proposed

**Date:** 2026-10-03

**Scope:** Turn the one 123k-line `librepaper` crate into a workspace of internal sub-crates along the seams the code already has, so that an edit recompiles the crate it touches and its dependents rather than everything. One user-facing `librepaper` binary and one `librepaper` entry point on crates.io remain. No behavior changes.

## Why

Agents run `cargo check`, `cargo build` and `cargo nextest run` after every edit. On a 16-core machine with a warm cache, one line changed inside a function in `storage/` costs today:

| command | wall time | CPU |
| --- | ---: | --- |
| `cargo check` | 27 s | 99%, one core |
| `cargo nextest run --no-run` | 19 s | 230% |
| `cargo nextest run -E 'test(/x/)'` (matches nothing) | 14 s | 200% |
| `cargo build` | 62 s | 140% |

The 99% is the diagnosis. A crate is the unit of compilation, so one crate means one rustc frontend on one thread no matter how many cores exist, and no matter how small the edit. Dependencies are already cached and already parallel; the application itself is the serial part. Splitting it is the only lever that touches this number. Everything else (mold, line-tables-only debuginfo, offline sqlx) is already in place.

The split also isolates two inputs that invalidate the whole crate today: `web/dist`, compiled in by `include_dir!` in `server/shell.rs`, and `.sqlx`, read by the query macros in `storage/`. After the split each one invalidates one small crate.

## Non-goals

- No change in behavior, output, protocol, schema or on-disk format.
- No new public API. The sub-crates are internal; the `librepaper` crate's re-exports stay the only supported surface.
- No second language. `SPEC-go-migration.md` is a separate proposal; this one removes the premise it rests on.
- No change to the release binary, cargo-dist targets, installers or the `cargo install librepaper` path.

## The dependency graph today

Counted as `crate::<module>` references between top-level modules of `crates/librepaper/src` (production and test code together):

```text
                 agen assi auth auto  cli conf docu http loca  log priv quar resu room seed serv stor  tls util
agent_query        .    -    -    -    -    -   13    -    -    -    -    -    -    -    -    -    -    -    -
assistant          -    .    3    3    -    -    -    -    9    -    6    -    -    -    -    -    -    3    3
auth               -    -    .    -    -    -    -    2    -    -    -    -    -    -    -    -    -    -    -
automation         -    2    2    .    -    -    -    1    2    -    1    -    -    -    -    -    -    -    1
cli                -    1    2    4    .    3    1    2   13    -    2    -    -    -    1    3    9    -    2
config             -    -    -    -    -    .    -    -    -   10    -    -    -    -    -    2    -    -    -
document           -    -    2    -    -    3    .    -    -    2    -    -    1    -    -    -   15    -   11
local              -    7   14    2   23    -   10    -    .    -    3   14   41    -    -    -    -    -    9
log                -    -    1    -    -    4   26    -    -    .    -    -    -   14    -    -   23    -    -
room               7    -    1    -    -    4   26    -    2   36    -    -    -    .    -    6   46    -    8
seed               -    -    -    -    -    -    1    -    -    2    -    -    -    -    .    -    6    -    -
server             4    5   28    -    -   18   77    -    2   65    -    -    6  103    3    .  114    -   50
storage            -    -    7    -    -   31    8    -    -   81    -    -    -   10    -    2    .    -    3
```

Lines per module: server 27.1k, local 22.1k, storage 21.8k, room 16.0k, log 10.0k, assistant 5.2k, document 5.0k, auth 2.8k, agent_query 2.4k, cli 2.2k, automation 1.1k, config 0.9k, seed 0.9k, quarto 0.6k, results 0.4k, the rest under 200 each.

Three facts drive the design:

1. **The companion stack is already independent of the server stack.** `local`, `assistant` and `automation` reference `document` (projection types only), `auth` (four small helpers), `quarto`, `results`, `private_files` and `cli`. They never reference `storage`, `log`, `room` or `server`. That is 28k lines that can compile without Loro, SQLx, object_store or the room.
2. **`log` and `storage` are one unit.** `storage::worker` and `storage::postgres` return and consume `log` types (`Ingested`, `SnapshotMode`, `Sequencer`, `Batch`) and `log::sequencer` drives `storage::worker`, `PostgresCatalog` and `BlobStore`. The 81 and 23 references go both ways through production code. They stay together.
3. **Every other cycle is a handful of misplaced items,** listed below with where each one goes.

## Proposed crates

Bottom-up, with the external dependencies that land in each:

| crate | contents | approx. lines | heavy deps |
| --- | --- | ---: | --- |
| `librepaper-base` | `util`, `http`, `tls`, `config`, `auth`, `private_files`, plus `socket_budget` from `server` and the budget formulas from `log` | 4.3k | axum (auth extractors), sha2, hmac |
| `librepaper-document` | `document` without `store.rs`, plus `results`, `quarto` | 4.0k | loro |
| `librepaper-engine` | `log`, `storage`, `document/store.rs` as `engine::store`, the `annotation` and `outgoing` data types from `room` | 30k | loro, sqlx, object_store, zstd, tar |
| `librepaper-room` | `room`, `agent_query`, the proposal validators from `server::mcp::comments` | 18k | loro |
| `librepaper-shell` | `server/shell.rs` only: the `include_dir!` of the built browser app | 0.1k | include_dir |
| `librepaper-server` | the rest of `server` | 27k | axum, rmcp, tokio-tungstenite |
| `librepaper-companion` | `local`, `assistant`, `automation`, plus the credential helpers and `Local*` clap types from `cli` | 28k | axum, agent-client-protocol, rmcp, the Linux dialog stack, rfd |
| `librepaper` | `lib.rs` facade re-exports, `cli`, `seed`, `main.rs`, `tests/`, the whole-stack benchmarks | 7k | clap |
| `librepaper-testing` | not a crate: the shared Postgres harness is `engine::testing`, compiled always (it needs the catalogue types, so a crate beside `engine` would be a cycle), and the one fixture moved to `web/tests/fixtures/quarto` | 0.2k | sqlx |

Dependency graph, which must stay acyclic and is enforced by Cargo:

```text
librepaper ──┬──> server ──┬──> room ──> engine ──> document ──> base
             │             ├──> shell
             │             └──> engine, document, base
             ├──> companion ──> document, base
             ├──> room, engine, document, base
             └──> shell
```

`companion` and `server` share nothing above `document`. An edit under `local/` recompiles `companion` and relinks the binary; `engine`, `room` and `server` are untouched. An edit under `server/` leaves `companion`, `room` and `engine` alone. An edit under `storage/` recompiles `engine`, `room`, `server` and the facade, which is the worst case and still skips 32k lines of companion code and runs the untouched crates' dependents in parallel where the graph allows.

Why not finer:

- `log` and `storage` split would need a types-only crate for `Ingested`, `SnapshotMode`, `Batch`, `Head` and friends, and `worker.rs` would move into `log`. Possible later; not worth doing before the first measurements.
- `server/mcp` (5.1k with `mcp.rs`) and `server/chat` (1.6k) are candidates for a second round if `server` turns out to be the crate agents wait on most. It has 138 commits since August, the most of any module, so this is likely.
- `assistant` and `automation` are a cycle (`assistant -> automation::peer`, `automation -> assistant::journal`) and `local` and `assistant` are a cycle (`SessionRegistry` one way, `ConnectionStore` the other). One crate until someone needs otherwise.

Why not coarser: `base` and `document` exist so that `companion` does not depend on `engine`. Merging either into `engine` would drag SQLx and the log into every companion build.

## The seams to cut, item by item

Each item is a move or an inversion that the current single crate accepts today, so each can land as its own small commit before any crate boundary exists. That is the point: by the time a crate is extracted, the extraction is a file move and a `use` rewrite.

### Into `base`

- `server::socket_budget::{SocketBudget, SocketPolicy}` moves to `base::socket_budget`. Used by `config.rs` (two references) and `room::outgoing` (four). `config` cannot depend on `server`.
- `log::budget::{estimate, DEFAULT_EXPANSION, BUILD_TRANSIENT_EXPANSION}`, `log::sequencer::{max_row_bytes, max_pending_charge, BUFFER_CEILING_BYTES}` and `log::pending::scratch_for` move to `base::config::budget`. These are pure functions and constants that `config.rs` uses to derive and validate defaults (lines 368 to 732). `log` imports them back from `base`. `config` cannot depend on `log`.
- `auth::{random_bytes, random_token, now_unix}` move to `base::util`. They are the only reason `document`, `log` and `companion` reference `auth`; `auth` keeps the identity, policy and cookie code. `AGENT_GRANT_PREFIX` stays in `auth`, and `companion` depends on `base` anyway.

### Into `document`

- Nothing moves in. `document/store.rs` and `document/store_import_tests.rs` move out to `engine::store`: `Store` is "catalogue-backed document metadata, source reads, and mutations", it imports `PostgresCatalog`, `BlobStore` and the log `Registry`, and belongs beside them. The facade keeps `pub use engine::store::Store` so `librepaper::Store` is unchanged.
- `document/paths.rs` takes the one `Configuration` field it reads as a parameter instead of the whole struct, or `base` stays a dependency of `document` (it is either way, for `util`). Either is fine; the point is that `document` must not reference `log` or `storage`.
- `agent_query` stays out of `document` and goes to `room`, its only consumers being `room` (7) and `server` (4).

### Into `engine`

- `room::annotation::{CommentTarget, OriginalAnchor, PresentationContext}` move to `engine::annotation`. `storage::postgres::annotations` stores them; the row types and the anchor types belong together. `room` and the facade re-export them so `librepaper::annotation` is unchanged.
- `room::outgoing` (the `Sender`, `Receiver`, `Outgoing` channel types, 495 lines) moves to `engine::outgoing`. `log::recovery` and `storage::postgres` use the channel types; `server::Viewer`, which `outgoing.rs` names, is the one thing to check: if it is a data type, it moves with `outgoing`; if it carries server behavior, `outgoing` takes a trait or the plain fields it needs.
- `log::recovery` line 74 imports `Rooms` and `AddComment` so that recovered comments reach connected rooms. Invert it: recovery returns the recovered comments and the caller, which lives in `room` or `server`, applies them to `Rooms`. The test helpers at lines 399 to 420 and 874 build channels and a `CommentAuthor`; they move with the tests that need them or use `engine::outgoing` directly.
- `storage/postgres/benchmarks.rs` (3.7k lines, test only) constructs `Server::new` and rooms. It is a whole-stack benchmark and moves to `crates/librepaper/tests/` beside `frugal_mixed_bench.rs` and `writer_socket_bench.rs`, where it can see every crate.
- `storage::worker::Worker` and `Handle` stay in `engine` and are re-exported as `librepaper::worker`.

### Into `room`

- `server::mcp::comments::{pending_proposal, validate_existing_action}` move to `room::proposals`, their only non-server caller being `room::comments`. `server::mcp` calls them there.
- `agent_query` moves in as `room::agent_query`.

### Into `companion`

- `cli::{stored_token_at, store_token_at, state_home}` move to `companion::credentials` and `companion::paths` (the latter already exists as `local::paths`). `local` references them 19 times; `cli` keeps calling them through `companion`.
- `cli::{LocalArgs, LocalCommand, LocalAgentCommand}` (clap derive types) move next to `local/cli.rs`, which is where `run(LocalArgs)` lives. `clap` becomes a dependency of `companion`, which is cheap. The top-level `Command` enum in `cli/mod.rs` embeds them with `#[command(flatten)]` or a subcommand as today.
- `assistant/guidance.rs` keeps its `include_dir!("skills")`; the path becomes relative to the companion crate (see Packaging).

### Into `shell`

- `server/shell.rs` becomes the whole crate: the `include_dir!` static, the `include_str!` of `assets.lock` and `ShellFile`. `server` depends on it. The facade keeps `pub use librepaper_shell::ShellFile`.
- Its `build.rs` resolves `web/dist` and passes it to the code as `LIBREPAPER_SHELL_DIST` (`include_dir!("$LIBREPAPER_SHELL_DIST")`), watches every file under it, and copies `assets.lock` into `OUT_DIR`. Nothing else.

### Stays in the facade crate

- `cli/` (minus the pieces above), `seed/` (one caller, `cli`; one `sqlx::query!` in `seed/activity.rs`, so the facade keeps a `.sqlx` entry or `seed` moves to `engine`; prefer moving `seed` to `engine::seed` so only one crate reads `.sqlx`), `main.rs`, the seven integration tests under `tests/`, the whole-stack benchmarks.
- `lib.rs` keeps every current re-export, now pointing at sub-crates. `crates/librepaper/tests/*.rs` and `tools/fuzz` use only `librepaper::{projection, postgres, worker, config, session, paths, log, source_archive, protocol, locate, annotation}` and keep compiling unchanged.

### Shared test support

`src/tests/mod.rs` was the opt-in Postgres connection, migration and reset shared by the ignored database tests in `storage`, `log`, `room`, `server` and `seed`. It lives in `engine` as `engine::testing`, a `pub mod` compiled always rather than under `cfg(test)`, because the tests of `room` and `server` reach it across a crate boundary and `cfg(test)` does not cross one. A separate `librepaper-testing` crate would need the catalogue types from `engine` while `engine`'s own tests need the harness, which is a cycle. `testing` also holds `Outbox`, the update-batch source the `log` and `room` tests share. The unreferenced `src/tests/local` was deleted; the Quarto fixture moved beside the web test that reads it. The flattener's `TESTING` handling only acts when a crate named `librepaper-testing` exists, so it stays dormant.

## Workspace layout

```text
Cargo.toml                       workspace root: members, [workspace.package], [workspace.dependencies], profiles, dist metadata
crates/librepaper/               the published entry point: facade lib + bin + integration tests
crates/librepaper-base/
crates/librepaper-document/
crates/librepaper-engine/        owns migrations/ and .sqlx/
crates/librepaper-room/
crates/librepaper-shell/         owns the staged browser build
crates/librepaper-server/
crates/librepaper-companion/     owns the skills bundle path
tools/flatten/                   the lint and the generator for the published single crate, plus its build.rs
target/flat/                     generated, never committed: the package that gets published
```

- `[workspace.package]` holds `version`, `edition`, `license`, `repository`; every crate uses `version.workspace = true` so there is exactly one version number in the repository. The tag check in `publish-crates.yml` reads it from `[workspace.package]` instead of `[package]`.
- `[workspace.dependencies]` holds every external dependency with its version and features once; crates write `tokio.workspace = true`. This is what keeps the dependency graph identical before and after the split, and `Cargo.lock` should not change at all in the scaffolding stage.
- `[profile.*]`, `.cargo/config.toml` (mold, `SQLX_OFFLINE`) and `[workspace.metadata.dist]` stay at the root. `[package.metadata.dist] dist = true` stays on `crates/librepaper` only; cargo-dist builds one binary as before.
- Sub-crates set `[lib] doctest = false` like today and carry a one-paragraph `//!` saying they are internal, have no stable API and are versioned in lockstep with `librepaper`.
- Visibility: items that were `pub(crate)` and are used across a new boundary become `pub`. The sub-crate's `pub` is not a promise; the facade decides what `librepaper::` exposes.

## Build scripts, generated inputs and packaging

Today one `build.rs` watches `assets.lock`, `.sqlx`, `skills/` and `web/dist`. Cargo packages only files under a package's own directory, so each input has to live under, or be staged under, the crate that reads it.

| input | reader | after the split |
| --- | --- | --- |
| `web/dist` | `shell` | Stays where Vite builds it. The shell crate's `build.rs` canonicalizes `CARGO_MANIFEST_DIR/../../web/dist`, emits `cargo:rustc-env=LIBREPAPER_SHELL_DIST=<abs path>` and watches each file. The code does `include_dir!("$LIBREPAPER_SHELL_DIST")`. The flattener stages `dist/` at publish time, and the flat `build.rs` sets the same variable from it. None of the 16 `web/dist` references move. |
| `.sqlx` | `engine` (which holds `seed`) | Moved to `crates/librepaper-engine/.sqlx` in Stage 5; the root `.sqlx` and `SQLX_OFFLINE_DIR` are gone. `tools/db sqlx-prepare` and `sqlx-check` use `--workspace`; sqlx writes each crate's cache under that crate. `SQLX_OFFLINE` handling stays in the root `.cargo/config.toml` and in the engine crate's `build.rs` for packaged builds. |
| `migrations/postgres` | `engine` | Moves with the SQL. |
| `skills/` | `companion` | Either moves to `crates/librepaper-companion/skills` or the release workflow stages a copy there. Prefer the move; `skills/` has no other reader that cares where it is except the Makefile's source list. |
| `assets.lock` | `shell` (`include_str!` of the pinned wasm digests, `shell.rs:14`) | Stays at the repository root for the Node tools that read and write it (`web/tools/pin-tools`). The shell crate's `build.rs` copies it into `OUT_DIR` and the code reads `include_str!(concat!(env!("OUT_DIR"), "/assets.lock"))`, so the source path does not depend on the layout. The flattener copies the root file next to the flat `Cargo.toml`, where the flat `build.rs` does the same copy. |
| `LIBREPAPER_VERSION` | the facade (`VERSION`) | Unchanged. |

Each crate's `include` allowlist lists only its own files. The root `include` list in today's `Cargo.toml` is split accordingly.

## Publishing: one package, generated from the workspace

The intent is one `librepaper` package on crates.io and nothing else. Cargo does not allow a published package to depend on an unpublished path crate: `cargo publish` rejects a path dependency without a `version`, and the registry rejects a version whose dependencies it does not have. So the workspace is the development layout only, and the published package is a single crate that a tool generates from it. Every sub-crate is `publish = false`. Nothing is reserved on crates.io.

The alternative, publishing the sub-crates as `librepaper-*` with exact pins and `cargo publish --workspace`, is what Cargo is built for and would need none of the machinery below. It was rejected because it puts eight entries on crates.io for one product. If the flattener turns out to cost more than it saves, that alternative is the fallback and needs no code change beyond flipping `publish` and adding version pins.

### `tools/flatten`

A script, checked in and run by CI on every pull request, that produces a self-contained single-crate package directory under `target/flat/` from the workspace:

1. **Sources.** For each sub-crate `librepaper-<name>`, copy `src/` to `target/flat/src/<name>/`, with the crate's `lib.rs` becoming `src/<name>/mod.rs`. Copy the facade crate's `src/`, `tests/` and `build.rs` to the root. 2. **Paths.** In every copied sub-crate file, rewrite `\bcrate::` to `crate::<name>::` and `\blibrepaper_(\w+)::` to `crate::$1::`. In the facade's own files rewrite only the second pattern, because the facade is the root. Append `mod base; mod document; mod engine; mod room; mod shell; mod server; mod companion; #[cfg(test)] mod testing;` to the facade's `lib.rs`. The modules are private at the root, so the facade's re-exports remain the only public surface, which is tighter than the development layout where every sub-crate `pub` is visible.
3. **Inputs.** Copy each sub-crate's non-source inputs to the flat root at the same manifest-relative path: `dist/` and `assets.lock` from `shell`, `.sqlx/` and `migrations/` from `engine`, `skills/` from `companion`. Every `include_dir!`, `include_str!`, `sqlx::migrate!` and `env!("CARGO_MANIFEST_DIR")` path then resolves identically in both layouts.
4. **Manifest.** Generate `Cargo.toml` from the root manifest: `[package]` from `[workspace.package]` plus the facade's metadata, `[dependencies]` as the union of every sub-crate's dependencies resolved against `[workspace.dependencies]`, `[target.*.dependencies]` and `[dev-dependencies]` likewise, `[[test]]` entries from the facade, profiles copied, `include` listing the staged inputs. Copy `Cargo.lock`, then run `cargo metadata --offline` once so Cargo prunes the `librepaper-*` entries, and fail if anything else in the lockfile changed.
5. **Build script.** The flat `build.rs` is a checked-in file, `tools/flatten/build.rs`, that does what today's single build script does: watch `dist/` and set `LIBREPAPER_SHELL_DIST` to it, `assets.lock`, `.sqlx`, `skills/`, set `SQLX_OFFLINE`. The sub-crates' individual build scripts are the pieces of it.

The output is a normal crate. `cargo publish --dry-run --locked` on it, `cargo test` on it and `cargo install --path` on it are the acceptance tests, and the publish workflow runs `cargo publish` from that directory instead of the workspace.

### Rules the sub-crates follow so the transform stays mechanical

These are checked by `tools/flatten --lint`, which runs in CI before the transform and fails the build with the file and line:

- Sibling crates are referenced only as full paths, `librepaper_engine::store::Store`. No `use librepaper_engine;`, no `use librepaper_engine as engine;`, no `extern crate`. A `use librepaper_engine::{a, b};` is fine.
- No inner attributes in a sub-crate's `lib.rs` except `//!` documentation. `#![doc]`, `#![allow]`, `#![deny]` and feature gates belong on the workspace or on items.
- No `$crate` in `macro_rules!`. A macro that needs an absolute path spells the sibling path and the lint rewrites it like any other.
- No `env!("CARGO_PKG_NAME")`, `CARGO_PKG_VERSION` or `CARGO_CRATE_NAME` in a sub-crate. `VERSION` lives in the facade, which keeps its name in both layouts.
- Manifest-relative input paths are unique across sub-crates, so copying them to one root cannot collide. The lint checks the set of top-level entries each crate's `include` names.
- No sub-crate module named like another sub-crate, and no sub-crate named `cli`, `seed`, `tests` or anything else that is already a module of the facade.
- The string `crate::` does not appear in string literals or doc tests in sub-crates. There are none today; the lint keeps it so.

None of these rules is a burden in practice. They forbid things the code does not do. The point of writing them down is that an agent or a contributor who does one of them gets a lint failure with an explanation rather than a mysterious publish failure months later.

### Verification

- CI, every pull request that touches Rust, `Cargo.*`, `web/`, `skills/` or `.sqlx`: run the lint, run the flattener, then `cargo check --all-targets --locked` and `cargo test --lib --locked` inside `target/flat/`. The unit tests passing in the flat layout is the proof that the rewrite is semantically faithful, since every `#[cfg(test)]` module in every sub-crate is compiled and run as part of one crate.
- CI, same job: `cargo publish --dry-run --locked --allow-dirty` in `target/flat/`, which also exercises the `include` list and catches a file outside the package root.
- Release: the publish workflow builds the browser app as today, runs the flattener, and publishes from `target/flat/`. The tag check reads `[workspace.package].version`.
- A `cargo install librepaper --locked` smoke test from the published crate after each release, as a scheduled job or a manual step in `docs/releasing.md`.

### What a consumer sees

`cargo install librepaper`, docs.rs and anyone reading the published source see one crate whose `src/` has one directory per former sub-crate. Module paths in the published crate match the development layout one for one, so a file reference in an issue means the same thing in both. The `README.md` note that the repository builds as a workspace, and that the published crate is generated from it by `tools/flatten`, goes in `docs/releasing.md` and in a comment at the top of the generated `Cargo.toml`.

## Staged plan

Every stage ends with `cargo check --workspace --all-targets`, the full test suite, and the invalidation check below. No stage changes behavior. Each stage is one branch and one review.

### Stage 0: scaffolding, no code moves

- Introduce the workspace root manifest with `[workspace.package]`, `[workspace.dependencies]` and a single member, `crates/librepaper`, whose manifest moves into that directory.
- Keep the binary, tests, profiles and dist metadata exactly as they are.
- **Acceptance:** `Cargo.lock` unchanged. Release build identical in size and dependency list (`cargo tree` diff empty). CI green.

### Stage 1: shell

- Extract `librepaper-shell`. Its `build.rs` hands `web/dist` to the code through `LIBREPAPER_SHELL_DIST`; Vite's `outDir` and the 16 `web/dist` references do not change.
- **Acceptance:** `bun run build` followed by `cargo check -v` compiles `librepaper-shell` and relinks the binary, and compiles nothing else.

### Stage 2: the seams, still inside one crate

Land the moves listed under "The seams to cut" as separate small commits: `socket_budget` and budget formulas to `config`; `random_bytes` and friends to `util`; `store.rs` beside storage; `annotation` and `outgoing` beside storage; recovery inversion; proposal validators to `room`; credential helpers and `Local*` clap types beside `local`; `agent_query` under `room`; benchmarks to `tests/`; `seed` beside storage.

- **Acceptance:** the reference matrix above, recomputed, shows zero references from `config` to `log` or `server`, from `document` to `storage` or `log`, from `log` or `storage` to `room` or `server`, from `room` to `server`, and from `local`, `assistant` or `automation` to `cli`. The script that computes the matrix lands under `tools/` so the check can be rerun.

### Stage 3: base and document

- Extract `librepaper-base`, then `librepaper-document`.
- **Acceptance:** an edit in `document/` does not compile `base`; an edit in `base` compiles everything, which is expected for the bottom crate and is why it is small.

### Stage 4: companion

- Extract `librepaper-companion` with `local`, `assistant`, `automation`. Move or stage `skills/`. The Linux dialog target dependencies and `rfd` move to its manifest.
- **Acceptance:** an edit under `local/` compiles `librepaper-companion` and `librepaper` only, verified with `cargo check -v`. Record the wall time of `cargo check` after a one-line edit in `local/service/`, against today's 27 s.

### Stage 5: engine

- Extract `librepaper-engine` with `log`, `storage`, `store`, `annotation`, `outgoing`, `seed`. Move `.sqlx` and `migrations`. The Postgres test harness becomes `engine::testing`, not a crate.
- **Acceptance:** `tools/db sqlx-check` passes with the moved cache. An edit in `engine` does not compile `companion`. The ignored Postgres tests run with `cargo test -p librepaper-engine --lib -- --ignored --test-threads=1`, and the CI job that runs them is updated to name every crate that has them.

### Stage 6: room, then server

- Extract `librepaper-room` with `agent_query`, then `librepaper-server`.
- **Acceptance:** an edit in `server/` compiles `librepaper-server` and `librepaper` only. An edit in `room/` compiles `room`, `server` and `librepaper`. Record both timings.

### Stage 7: the flattener and release plumbing

This stage can start as soon as Stage 1 exists, because a two-crate workspace is enough to develop the tool against, and it must be finished before any tag is pushed from a multi-crate tree.

- Write `tools/flatten` with its `--lint` mode and the checked-in flat `build.rs`.
- Add the CI job: lint, flatten, `cargo check --all-targets --locked`, `cargo test --lib --locked` and `cargo publish --dry-run --locked --allow-dirty`, all inside `target/flat/`.
- Switch `publish-crates.yml` to flatten and publish from `target/flat/`; the tag check reads `[workspace.package].version`. Update `docs/releasing.md`.
- Update `docs/architecture/building.md` and `AGENTS.md`: agents run `cargo check -p <crate>` and `cargo nextest run -p <crate>` for the crate they edited, and the full workspace commands only before handing work back.
- **Acceptance:** the flat crate's unit tests pass; a tagged release publishes one `librepaper` crate; `cargo install librepaper --locked` from a clean machine produces a working binary; the cargo-dist artifacts are unchanged.

## Measurement

Before Stage 0, record on the development machine and in CI, each after a one-line edit inside a function:

- `cargo check` after an edit in `local/service/`, `server/`, `storage/postgres/`, `room/`;
- `cargo nextest run --no-run` after the same edits;
- `cargo build` after the same edits;
- a no-op `cargo check` and a no-op `cargo nextest run --no-run`;
- `cargo check -v` after each edit, keeping only the `Compiling` lines, which is the invalidation evidence.

Repeat after Stages 4 and 6. The success condition is relative, not a fixed number: the edit's `Compiling` set is the crate touched and its dependents, nothing else, and the wall time for a companion or server edit is a fraction of today's 27 s rather than all of it. If it is not, the crate graph is wrong and the stage is reverted before the next one starts.

Test binaries go from eight to roughly fifteen. nextest lists tests by running each binary, so a larger count is not free; but each sub-crate's test binary links only its own subtree, and the seven integration binaries in `crates/librepaper/tests/` are a separate, pre-existing cost. Folding those seven into two or three is a worthwhile follow-up and is out of scope here.

## Risks and traps

- **`pub(crate)` erosion.** Making items `pub` to cross a boundary widens what the sub-crate exposes. Acceptable because the sub-crates are internal, but the facade's re-export list, not the sub-crates' visibility, remains the definition of the public API, and `tests/` plus `tools/fuzz` should keep compiling against the facade only.
- **`include_dir!` and `$CARGO_MANIFEST_DIR`.** Each crate's manifest dir is different now. Every `include_dir!`, `include_str!`, `env!("CARGO_MANIFEST_DIR")` and `build.rs` path has to be re-rooted; a wrong path compiles in an empty directory and shows up as a blank shell page, not a build error. Stage 1's acceptance exists to catch exactly this once.
- **sqlx offline cache location.** sqlx reads `.sqlx` from the crate being compiled, or from `SQLX_OFFLINE_DIR`. A stale cache in the old location would be silently ignored with `SQLX_OFFLINE=true` and fail only in the `sqlx-check` CI job. Delete the old directory in Stage 5, do not leave both.
- **Feature unification.** With `[workspace.dependencies]` features are declared once; a crate that enables an extra feature on a shared dependency changes the build for every crate. Keep feature lists at the workspace level.
- **Worktrees and `target/`.** Agents work in worktrees. Each worktree has its own `target/`, so the cold build cost is paid per worktree as today; the split does not change that, only the incremental cost after it.
- **The `lp_` token.** Crate names use the `librepaper-` prefix; nothing introduces an `lp` abbreviation (see the rename notes).
- **Cargo.lock in the published package.** `cargo install --locked` uses the lockfile shipped with the crate. The flattener copies the workspace lockfile and lets Cargo prune the `librepaper-*` entries; it fails if any other line changes, so the published resolution is the tested one.
- **The two layouts drifting.** The flattener is the only place that knows both. It is exercised on every pull request, not only at release, and the flat crate's own unit tests are the oracle. A rewrite bug that compiles but changes meaning is the case to fear; the lint rules above exist to make the rewrite purely syntactic, and the `crate::` prefix rule means a missed rewrite is a compile error, not a silent change.
- **Rewriting too much.** `\bcrate::` also matches inside doc comments and, in principle, string literals. Doc comments are harmless. The lint forbids the string case rather than teaching the rewriter about Rust lexing.

## Open decisions

1. Whether the flattener is written in the repository's shell-and-Node tooling style (`tools/`) or as a small Rust binary under `tools/flatten` excluded from the workspace like `tools/measure`. Node matches the other release tooling; Rust can reuse `cargo metadata` output directly. Either is fine; pick one in Stage 7.
2. Whether `seed` goes to `engine` (one `.sqlx` reader) or stays in the facade with its own cache entry. This spec says `engine`.
3. Whether to split `server/mcp` and `server/chat` out of `server` in a second round, decided by the Stage 6 timings.
4. Whether `document/paths.rs` keeps `Configuration` as a parameter or takes the single field. Either keeps `document` independent of `engine`.

## References

- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html): `[workspace.package]`, `[workspace.dependencies]`, member inheritance.
- [Cargo publish, workspace publishing](https://doc.rust-lang.org/cargo/commands/cargo-publish.html): `--workspace`, dependency ordering, path dependencies with `version`.
- [cargo package, included files](https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields): only files under the package root are packaged.
- [sqlx offline mode](https://docs.rs/sqlx/latest/sqlx/macro.query.html#offline-mode): `.sqlx` per crate, `SQLX_OFFLINE_DIR`.
- `SPEC-go-migration.md`: the proposal this one replaces as the answer to compile time.
