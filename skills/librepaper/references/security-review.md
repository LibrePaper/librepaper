# Security review

A review of how someone could read or write another user's files. Read-only. The main agent maps the surface, fans out one reviewer per surface, then verifies every claim itself before reporting. Reviewers must not run cargo, bun, tests or servers, and must give file:line, preconditions, a request sequence and CONFIRMED or SUSPECTED for each finding.

Start from the trust model in `docs/architecture/index.md` (Trust model): where code runs, which origin holds what, and what each side may trust.

## Where authority is decided

- `crates/librepaper-server/src/server/mod.rs`: `Viewer`, `viewer_as`, `resolved_role`, `automation_role`, `ceiling_for`, `link_hash`, `check_readable`, `entry_at_least_in`. Every gate goes through `viewer_as`.
- `crates/librepaper-server/src/server/routes.rs`: the route table. Build a table of route, rung required, where enforced.
- `crates/librepaper-engine/src/storage/store.rs` `get_result` and `postgres/repository.rs` `document_by_slug`: what a slug resolves to. The only `status != "active"` check is `Rooms::get` in `crates/librepaper-room/src/room/mod.rs`; paths that skip the room skip it.
- `crates/librepaper-server/src/server/agent_auth.rs` and `librepaper-base/src/auth`: agent grants, device tokens, sessions.
- `crates/librepaper-server/src/server/origins.rs` `cross_site_refused`, `ws_origin_refused`; `reply.rs` `local_path` for sign-in redirects.
- `crates/librepaper-companion/src/local/service/mod.rs` `handle`, `dispatch`, `check_credential`; `desktop/src/main.rs` the `lp-local` handler and `trusted`.
- `crates/librepaper-companion/src/assistant/acp.rs` `bridge_environment` and `local/connections.rs`: what the local agent process can reach.
- `web/src/lib/frame-messages.js`, `components/Preview.svelte`: the hostile document frame.

## Surfaces to split between reviewers

1. Server authorization and every read route, including Postgres query scoping.
2. Desktop app and local service: lp-local reachability, path traversal, folder grants, Quarto execution.
3. Agent grants, MCP tools, assistant channel, the local agent's environment, git sync.
4. Web app HTML sinks, frame sandbox and postMessage, storage on the docs origin, link key storage.
5. Sign-in, sessions, device flow, CSRF, origins, admin console.
6. Blob storage keys, bucket exposure, backups, Caddy and compose, wasm integrity.

## Known by design, do not report

- The operator sees every draft and comment.
- An Editor on a shared document runs Quarto code on the author's machine.
- A document frame may contact third-party HTTPS hosts.
- Legacy "open" documents are readable by any signed-in account.

## Verifying a fix

- `web/tests/integration/agent-bridge.mjs` is the end-to-end test for the agent path: a real `librepaper mcp` bridge against a real `admin serve`. It needs the binary, a Postgres URL and the `LIBREPAPER_TEST_PSQL` pair. Read the test's header for the exact variables.
- `crates/librepaper-server/src/server/authz_http_tests.rs` holds the Postgres-gated authorization cases (trashed documents, automation refusals, listing cursor). `tools/dev/db test` runs them; grep its log for `authz_http_tests` to prove they ran.
- Run the Rust gate before any binary-backed node test, never concurrently: a rebuild rewrites `target/debug/librepaper` under the tests.
- Eleven browser tests mock `docs_origin: location.origin`; a client-side refusal of a same-origin docs frame breaks the harness, and the server already refuses that configuration in `origins.rs`.
