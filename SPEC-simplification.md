# Specification: Rust workspace simplification

**Status: proposed, document only.** This document records verified findings and recommended packages. It does not authorize code changes by itself. Revised 2026-10-24 against `a6abe5f0` by reading the code. Nothing was built or run for this revision.

**Sources.** An earlier draft consolidated two source plans, `SPEC-simplification-opus.md` and `SPEC-simplification-sol.md`. Neither file exists in the tree, in git history, or on the `docs/spec-simplification-*` branches, so this revision drops their identifiers and keeps only claims that were checked against the code. Line numbers below are from `a6abe5f0` and drift with every merge. Refresh them at the implementation head.

## 1. Purpose and the one decision this document needs

Reduce repeated representations and redundant work in the document engine, the server and room boundary, the companion service, the CLI and the desktop shell. Use canonical state directly. Keep an abstraction only where it enforces an invariant, provides a test seam with a real alternate implementation, or defines a protocol boundary. Add no generic framework.

The earlier draft hedged every removal on an "unreleased" argument and asked for a retention plan per item. Both sides of that were stale. The repository rule is hard cutover with no shims and no migrations, and the official instance has been live since 2026-10-01 and runs v0.0.50, so persisted data does exist. One decision from the owner replaces the per-item conditions.

**Does hard cutover still apply to persisted formats?** The formats in question are desktop job records on disk (P8), legacy anchor records and the room-open backfill (P4), and `open` ownership rows (Section 5).

- If yes, the legacy decoders and the backfill are deleted outright, and the SQL `open` clause goes with its column.
- If only server-side data is protected, one query against production for `open` rows and legacy anchor records decides what stays. Desktop job history is cut over either way, because nothing reads the duplicated fields.

Everything else in this document is behavior-preserving cleanup or a verified bug and needs no decision.

## 2. Design rules

1. **One canonical authority per fact.** The ordered log is source history. Anchor records identify passages. `Projected` is derived from the current Loro head. Database comment records own discussion content and permissions. Do not add mutable copies of ranges, file lists or log-head state.
2. **Authorization is decided inside the fenced transaction.** `begin_document_command` runs `check_document_command` right after the transaction begins and after the writer-epoch check (crates/librepaper-engine/src/storage/postgres/document_log.rs:290-313). That check is the authority. A probe or a middleware context is an early answer only.
3. **Retries stay durable and scoped.** Replay lookup runs before source preparation or row insertion. Retry scope is bound to the actor, never to a mutable credential.
4. **Distinct budgets stay distinct.** Pending-memory reservations, durable quota reservations, decoded-document reservations and fork reservations cover different resources and lifetimes.
5. **The sequencer owns per-document ordering,** replay, semantic-command transactions, buffered writes, relay and cache installation.
6. **Paired reads use one lock.** A result that combines a `LoroDoc` and its projection comes from `with_projected_head`. Projection-only reads use `Sequencer::projection`.
7. **Policy boundaries are not interchangeable** because their code looks alike. Path validators, HTTP client policies, consent checks and protocol contracts stay separate.
8. **No new generic framework.** Every helper needs a concrete contract and callers that exist.

## 3. Packages, in order of value

Each package has one file boundary and is reviewable alone. "Owner answer" means the Section 1 decision.

### P0. Quarto builds must not wait for native tool discovery

`run` in crates/librepaper-companion/src/local/engine_adapter.rs:36 calls `discovery::tool_paths()` before the `builder == "quarto"` branch at line 56 returns. `tool_paths` covers only typst and calepin (discovery.rs:32) and rediscovers when `tools.json` is missing or stale, with a 10 s probe timeout (discovery.rs:36). A Quarto job can therefore wait on probes it never uses. Move the call into the native branch at line 59. Fix the doc comment at discovery.rs:370-372, which says `tool_paths` never rescans.

**Acceptance.** A Quarto job never calls `tool_paths`. A native job still does. The capabilities aggregator (`discover` at discovery.rs:317) is unchanged.

### P1. Delete code with no callers

Each item was checked by whole-repo grep at `a6abe5f0`. Recheck at the implementation head, then delete, tests of the dead item included.

- `replace_annotation_authorized` (crates/librepaper-engine/src/storage/postgres/annotations.rs:496).
- `admit_account_upload` (postgres/repository.rs:541). Its only caller is a test at postgres/mod.rs:1226-1233.
- `prune_link_grants` (postgres/access.rs:221). Keep `prune_link_grants_tx` (access.rs:232), which `save_document_sharing` uses (repository.rs:357).
- `Accounts::lookup` (crates/librepaper-base/src/auth/github.rs:284-286) and the write-only `accounts` field on the server (crates/librepaper-server/src/server/mod.rs:268, set at 948 and 972). Only the tests at github.rs:505-506 call it.
- `Authority::deployment` (postgres/commit.rs:41-50). Its refusal was never structural. It came from both policy flags being false at annotations.rs:439-444.
- The unversioned-visitor upgrade at crates/librepaper-server/src/server/signin.rs:777-790. It is unreachable because `read_visitor` (crates/librepaper-base/src/auth/mod.rs:673-676) returns empty for any cookie without the `v1.` prefix. There is no legacy owner-key cookie branch anywhere. Owner role comes from account identity (store.rs:239-241).
- `unowned_publisher` on `MutationActor` (crates/librepaper-engine/src/storage/store.rs:538). Every shipped constructor passes false (documents.rs:576, 1431 and 1760, figures.rs:122, onboarding.rs:154 and source_change.rs:1000). The one true value (sharing.rs:259) goes to `modify_as_owner`, which refuses an empty account at store.rs:1662 and never creates a document. Deleting it makes the create branch at store.rs:1411 always `owned`. The SQL `open` clause (annotations.rs:442) stays until the owner answer, because old rows may hold it.
- `_owner_key` on store.rs:855 and the `who.key` argument `handle_list` passes.
- `projection_if_warm` (crates/librepaper-engine/src/log/sequencer.rs:2146). Its callers are log/tests.rs:1854 and 1889. Its doc at 2140-2143 names a re-anchoring pass that does not exist.
- The six unreachable origin refusals in crates/librepaper-companion/src/local/service/backups.rs (lines 1050, 1065, 1097, 1141, 1169, 1204). `authenticate` (service/mod.rs:1452-1465) already returns 403 unless the origin is valid.
- The second origin and manifest checks in `handle_jobs_post` (service/jobs.rs:389 and 392 repeat lines 286 and 289).
- The tray assets under crates/librepaper-companion/src/local/assets (render-tray-icon.sh, tray-outline.svg, tray-outline-64.png, tray-outline-64.rgba). Nothing references them. The desktop tray uses `app.default_window_icon()` (desktop/src/tray.rs:64-65).

Stale comments to fix in the same pass. Five comments name a `handle_mcp` that does not exist (mcp.rs:913, mcp/render.rs:399, mcp/comments.rs:410, mcp/operations.rs:564 and 1177). The real recheck is `mcp_recheck` from `handle_tool_call` (mcp.rs:387, 504). agent.rs:107-109 says `operation_scope` binds link, generation and role, which holds only on the MCP path. commit.rs:21-22 calls `principal_key` the peer key on batches. worker.rs:527-528 says three pages where the scan returns four lists of 64. engine_adapter.rs:1-9 says Quarto is the only engine on that surface. folder.rs:1 says consent names the document. backfill.rs:6-14 promises no build without comments. figures.rs:20-24 and room/mod.rs:119-121 call the upload map a quota claim of reserved bytes. server/mod.rs:401-403 and 2469-2471 say an empty caller owns an unowned entry. jobs.rs:157-158 and 311-315 say the protocol check runs nowhere else. annotations.rs:348-350 says the caller takes the writer fence first, which the probe does not.

### P2. Reuse the projection cache and drop the duplicate readability check

`Store::project_files` (store.rs:1156) calls `with_head` and recomputes `projection::project` at line 1175. `Sequencer::projection` (sequencer.rs:2168) already memoizes the same computation with the same rules into `Inner.projection`. Call it instead. `with_head` never fills that memo, so every copy and export today recomputes the projection.

`Room::agent_query_snapshot` (crates/librepaper-room/src/room/agent_view.rs:24) checks `unreadable` before calling `projection()`, whose sequencer body checks `readable()` at sequencer.rs:2172. Remove the pre-check and map the projection error. `readable()` also refuses a fenced document, which the module doc at agent_view.rs:5-7 omits.

**Acceptance.** Same projected paths, collision winners and absent-asset skips. No sequencer change is needed, so this package does not overlap P3.

### P3. Charge the command row to the ledger

`flush` retires and then charges (sequencer.rs:1571-1572). `command_reporting_replay` retires at 2098-2101 and never charges. The only `ledger.charge` calls are at 1123 (chat), 1572 (flush) and 2667 (compaction). `wrote_a_row` is the right key, because an empty `acknowledged` skips the insert (2007-2009, 2031-2057). Add one charge of `row_bytes` after the commit when `wrote_a_row` is true. None on replay, rollback or a no-row command.

The ledger is soft admission (log/ledger.rs:1-5), refreshed from the catalog only at construction (sequencer.rs:984). Postgres enforces the durable quota. So this is an under-estimate that lasts until restart, not a quota hole. Keep it separate from any reservation redesign.

### P4. Verified behavior bugs

Each is a separate change with its own test.

- **`/project` caching.** `handle_project` sets an ETag and `private, no-cache` (documents.rs:1212, 1231). The cost middleware then overwrites Cache-Control to `private, no-store` on every `/api/` path (cost.rs:529-534, through `set` at reply.rs:222-226). The 304 path helps only a client that keeps its own ETag. Check whether the companion backup fetch at backup.rs:522 sends `If-None-Match`. Then either exempt ETag routes from the blanket header or drop the ETag logic.
- **Calepin override ignored by native builds.** `integrations::executable` (integrations.rs:194-208) resolves the env var, then the settings path, then PATH, and capabilities and previews use it (preview/calepin.rs:120-124). The native plan (builders/mod.rs:119-126) uses it only when a settings path is set and otherwise takes the discovery map, which `find_tool` fills from PATH alone (discovery.rs:107-116). With `LIBREPAPER_CALEPIN_PATH` set and no settings path, capabilities report a binary and the build fails with tool-missing. Resolve the native plan through the same function. Typst has no override at all (builders/mod.rs:165).
- **Cold build on every room open.** `Rooms::get` (room/mod.rs:536) calls `backfill_anchors`, whose first line reads `with_head(anchors::has_legacy_records)` (room/backfill.rs:259). That builds the document on every cold open, with or without comments, against the module doc at backfill.rs:6-14. Under hard cutover, delete `backfill_anchors`. Otherwise read the comment page (backfill.rs:270) first and touch the head only when a comment exists. `HealAnchors` (backfill.rs:129-174) stays. Its only caller is `handle_history_trim` (server/history.rs:1045) through `Room::heal_comment_anchors` (room/mod.rs:336).
- **Reader-link export.** `librepaper export` fetches `/snapshot` (crates/librepaper/src/cli/export.rs:127), which returns 404 below Editor (documents.rs:1255), while the `--key` help (crates/librepaper/src/cli/mod.rs:95-96) says the link is read as its holder. Confirm by running it, then either fetch `/project` for reader links or fix the help.
- **Misnamed refusal.** MCP admission is a concurrency cap (capacity.rs:9-10, `GLOBAL` 8 and `PER_PRINCIPAL` 2) but the wire code is `rate_limited` (capacity.rs:65). Rename the code if no client matches on it. Replacing the cap with a rate limit is a policy change and is not proposed.

### P5. One typed authorization identity

**What exists.** `MutationActor` (store.rs:525) and `AgentAuthority` (room/agent.rs:101) carry account, session and link as `String`. `MutationAuthorization` (annotations.rs:122-130) and `Authority` (commit.rs:20) are typed, and `MutationAuthorization` calls the hash `token_hash` while the other two call it `link_hash`. Two conversions are lenient and turn parse failures into `None`: `MutationActor::authorization` (store.rs:545, 549, 551) and `Authority::mutation_authorization` (commit.rs:58-61). The server's `Viewer` does the same at server/mod.rs:533-536, 568 and 587. One conversion is strict and returns an error on a malformed nonempty value, `room::catalog::mutation_authorization` (room/catalog.rs:14-36). A lenient path treats a malformed account as a visitor at annotations.rs:442. The rename and sharing paths are backstopped by owner gates (store.rs:1662, repository.rs:362-384). The generic path is not.

**The round trip.** `Room::apply_source_change` (source_change.rs:969) builds a `MutationActor` literal (992-1000), converts strictly (1002), runs a standalone probe `authorize_document_mutation` in its own rolled-back transaction (1005), builds an `Authority` literal (1008-1016), and issues the command (1054). The sequencer then calls `begin_document_command` (sequencer.rs:1994), which converts leniently (commit.rs:53) and runs `authorize_mutation` again (document_log.rs:310-312). Every source change is authorized twice. `MutationActor::authorization` (store.rs:542-556) converts in the opposite order, and figures.rs:133 and 188 in a third.

**Proposal.** Parse account, session and link once at the authenticated boundary into typed fields, strictly. Pass that one value to the command. Delete the standalone probe at source_change.rs:1005 once the in-transaction conversion is strict, so a malformed value refuses instead of widening. Pick one field name for the hash. Keep `operation_scope` as retry scope and `principal_key` where a result or audit record needs it.

**Facts the characterization must record.**

- Publish paths hard-code `policy_editor: true` (documents.rs:573, 1428, 1757) after the HTTP gate at server/mod.rs:1628, so the in-transaction check never re-tests the publisher ceiling for uploads, renames or copies. Preserving current behavior there means preserving the absence of a recheck. Decide whether that is intended.
- `ceiling_for` (server/mod.rs:1313-1324) applies `provider_configured` to edit only, and the same triple repeats at server/mod.rs:1628 and quota.rs:32.
- `operation_scope` is a SHA-256 over slug, author, account, key, link, generation and role on the MCP path (mcp.rs:191-197) and `sync:{document_id}:{principal}` on the sync path (sync_link.rs:424).
- An agent-token caller has three principal spellings. `Viewer::principal_key` gives `token:<id>` (server/mod.rs:546-548), the socket rate key is `account:<owner>` (socket.rs:309), and the source-change `Authority` carries the bare owner id (source_change.rs:1009-1011). SQL never reads `principal_key`. Its live uses are sync-link identity comparisons (sync_link.rs:255, 278, 372, 374, 459) and the token cost key (cost.rs:129).
- Regression coverage exists for grants, sessions, ceilings, link expiry and revocation, open-document visitors and automation in `mutation_authorization_uses_live_grants_sessions_ceilings_and_link_identity` (postgres/mod.rs:487, gated on `LIBREPAPER_TEST_POSTGRES_URL`). Erased accounts have no mutation-path test. Add one before touching the carriers.

**Acceptance.** The SQL decision still runs inside the fenced transaction. Malformed nonempty values refuse. Valid actors behave the same. The gated test above and the server unit tests at server/mod.rs:2207-2487 still pass, plus the new erased-account case.

### P6. Finish the request-context migration

**What exists.** `cost::middleware_inner` validates credentials and installs `RequestContext` (cost.rs:476, 513). `api_guard` (routes.rs:96-118) only reads it. It answers 404 for the docs host or 401 and 503 for a failed credential, and it is layered on `api_router` alone. The fallback dispatch repeats the gate inline for `/api/` and `/ws/` (routes.rs:743-752). The comment at server/mod.rs:796-799 overstates `api_guard`.

`entry_viewer` (server/mod.rs:777) re-resolves the bearer and cookie through `authenticated_identity` (mod.rs:1116, 1158). `entry_viewer_in` (mod.rs:802) trusts the context identity, though `viewer_as` still reads the Authorization header for agent tokens and grants (mod.rs:1439). Header-path callers are history.rs:785, mcp.rs:263 and 336, `authenticated_entry` (mod.rs:862), and through wrappers `handle_restore` (history.rs:884, 924), `handle_history_trim` (1005, 1021), `handle_chat_history` (documents.rs:340), `handle_chat` (chat/mod.rs:250) and `handle_chat_socket` (chat/mod.rs:53). Context-path callers are documents.rs:1044, figures.rs:82, agent_auth.rs:40, `entry_at_least_in` (mod.rs:828) and its users at history.rs:473, 561, 731 and figures.rs:30, 162. Rungs are a `Role` argument to `entry_at_least` (mod.rs:892) and `entry_at_least_in` (mod.rs:820). `handle_snapshot` checks its rung inline (documents.rs:1255).

`DocumentService` (mod.rs:118-127) has public `store`, `rooms` and `background`. It has no `Deref`. `Server` derefs to it (mod.rs:372-377), and production reaches `self.store` and `self.rooms` on roughly 120 lines. Leave that Deref alone.

**Proposal.** Migrate the HTTP routes that still take the header path to the context path, one group at a time. Keep a fresh resolver for the chat socket and other non-middleware transports. Before each group, record the response for absent, invalid, store-unavailable, insufficient-rung, hidden and missing cases, and keep it.

**Keep these rechecks.** The principal recheck around the body read (sync_link.rs:362, 373) and the recheck closure (457-462) that runs at source_change.rs:989. The stale-snapshot check at source_change.rs:249-251. `reauthorize_connection` (socket.rs:1103), which runs on a 2 s tick, on every mutation frame and on chat frames. `recheck_issuer`, which runs after the load await (render.rs:117, 435) and whose 503 is only the account lookup at render.rs:170.

### P7. Remove the exact forwarding aliases

`annotations::authorize_mutation` (annotations.rs:134-141) and `PostgresCatalog::authorize_annotation_mutation` (449-456) forward to `PostgresCatalog::authorize_mutation` (351). Callers of the free function are repository.rs:316, 354 and 1163. Callers of the method are the `*_authorized` wrappers at annotations.rs:471, 485, 505, 551, 578, 606, 631 and 678. Route them to the central method. `authorize_document_mutation` (464-474) is the probe P5 deletes. Every mutation keeps its authorization in the transaction that writes.

### P8. Quarto dispatch and the duplicated job options

`BuildInputs::Quarto` (protocol.rs:238-244) holds `options: QuartoJobOptions` and an `overrides` map. The map is filled at jobs.rs:202, persisted and recovered (service/mod.rs:410), and read only through `BuildInputs::options()` (protocol.rs:267) by the native runner (builders/runner.rs:92), which Quarto never reaches, and by one test (service/mod.rs:1801). Delete the field and the `builder_options` half of the legacy decoder (service/mod.rs:402-405, 410). Nothing changes at runtime.

The rest of the legacy decoder `recover_job_request` (service/mod.rs:389-416) rebuilds inputs for records written before the `inputs` field existed. It goes under the owner answer. Either way, keep `legacy_jobs_keep_outputs_and_interrupted_jobs_recover_without_reexecution` (service/mod.rs:1780) meaningful for whatever shape remains.

Move `QuartoInvocationPlan` (engine_adapter.rs:104-180) under local/quarto, next to its only users (quarto/mod.rs:20, 464). Delete the `quarto_capture` forwarder (engine_adapter.rs:183-189). Two of its three callers already call `quarto_capture::collect` directly (quarto/mod.rs:682 and 690, against 870).

Keep as contracts the `kind == "build"` shape check (protocol.rs:380-382), `JobRequest.kind` in status and persisted records (jobs.rs:461, service/mod.rs:376-381), `deadline_seconds` (protocol.rs:211-213), and the protocol version 2 checks at jobs.rs:158-161 and protocol.rs:377-379. `decode_preview` rewrites `kind` to `build` before validating (protocol.rs:541). Leave that or give the preview shape its own check, but do not let the rewrite disappear by accident.

### P9. Small duplications that pass the identical-semantics test

Share only what is byte-identical, keep the differing parts local, and add no generic layer.

- The limit and cursor parsers in `handle_list` and `handle_backup_projects` (routes.rs:1352-1367 and 1468-1483) are identical. A malformed limit silently becomes 200. Share the parser only.
- `counts_for`, `marks_for` and `tags_for` (store.rs:1229-1235, 1256-1262, 1285-1291) share one id-to-slug block. Account parsing differs between them (1254-1255 against 1284), so share the block, not the functions.
- `NewLabel` in agent.rs:639 and source_change.rs:864 differ only in `reason`. A helper may own the common fields.
- `record_map`, `str_of` and `bytes_of` are byte-identical in document/anchors.rs:822-843 and document/track.rs:686-715. `texts_of` and `text_ids_of` live in document/session/shape.rs (84, 108), and `texts_of` is fixture-only, so leave them.
- `page` and `thread_page` in room/comments.rs (707, 851) repeat the repeatable-read setup, the lookahead and the byte admission. Two call sites only. Share if it reads better.
- The six backup handlers keep `authenticate` and `account_id` after P1 removes the dead branch. A small gate helper is fine.

**Leave alone.** The two worker helpers differ (overflow keeps the rescan bit at worker.rs:237-239, due-rescan sets it at 257-261). Quarto and Calepin previews already share `verify_bound_manifest` (quarto/mod.rs:1649) and `refuse_escaping_entrypoint` (preview/mod.rs:107). The four path validators are different policies by design. The projection check (document/projection/paths.rs:65-109) has no colon or drive rule, `document/paths.rs:53-65` rejects a drive prefix and allows colons, `backup.rs:1090` rejects neither, and `export.rs:295-303` accepts only `Component::Normal`, which lets an empty path and a Unix backslash through. Fixing `export.rs` for the empty path is a candidate, not a sharing task. HTTP clients differ on purpose (`provider_client` never redirects at crates/librepaper-base/src/http.rs:39, `client()` follows at line 16, `read_capped` is 64 KiB at line 47). `StorageLimit` (config/mod.rs:158-162) and `StoragePolicy` (postgres/mod.rs:70-74) map at serve.rs:287-293 with the `usize` to `i64` cast bounded at server_config.rs:785-786. A filesystem or S3 enum for the string dispatch at storage/mod.rs:107 and 181 is optional. The companion state files (service.json, tools.json, integrations.json, connections.json with its lock, backups.json, git-grants.json, quarto-bindings.json, tokens.json with its lock) have distinct lifetimes and locks. Do not merge them.

## 4. Behaviors that must stay explicit

- `Command` names two things, the room wire enum (room/command.rs:11) and the engine transaction trait (sequencer.rs:392). A socket-frame refactor may replace the first and never the second.
- The socket reader drops malformed JSON and type-mismatched known frames without a reply (socket.rs:506-508). Unknown kinds that do not start with `doc-` get an error frame carrying `temp_id`, `request_id` and a nonempty `comment_id` (command.rs:231-236, 95-105). Unknown `doc-*` kinds are dropped silently (socket.rs:652-653, 996). A reader below Commenter gets a refusal without `comment_id` (server/mod.rs:1783-1800). A known frame missing `comment_id` gets a `Missing` error (command.rs:161-166). Preserve all five.
- `AddReply`, `ResolveComment` and `EditComment` (room/comments.rs:1310, 1410, 1582) ignore the head, yet `command_reporting_replay` materializes it for every command (sequencer.rs:1911-1929). A later `needs_head` optimization may skip the materialization only. Lock, replay, transaction, authorization and accounting stay. `AttachAsset` stays serialized (figures.rs:61-64, 208).
- `PendingBudget` (log/pending.rs:64) and `Budget` (log/budget.rs:45) differ in arithmetic, notify and eviction. Do not merge them. Ingest takes the durable reservation at sequencer.rs:1311 under the inner lock. Keep it.
- The worker keeps its overflow bit, deadlines, bounded queues (`QUEUE` 256 at worker.rs:68, `DEADLINES` 1024 at schedule.rs:22), startup passes (worker.rs:525, 594) and backoff (schedule.rs:42-49).
- `[limits]`, `[retention]` and the S3 session token are documented operator settings (server_config.rs:254-270 and 198, deploy/librepaper.toml:94-103). Only the session token lacks docs. The integration tests web/tests/integration/pending-budget-recovery.mjs:51-52 and slow-subscriber-recovery.mjs:18 set `log_quota_mib`, `pending_mib`, `pending_scratch_mib` and `session_peer_queue_frames`. Keep them configurable.
- Production calls `registry.compacts_through(background.clone())` (serve.rs:575). Most test fixtures do not (http_test_support.rs:50-66), so their compaction asks do nothing (sequencer.rs:1435, 1603, 2130). Characterize before calling that a bug.
- `LogCatalog` (sequencer.rs:528) has `FakeCatalog` (log/tests.rs:98) and `Runner` (service/mod.rs:93) has `FakeRunner` (148). Keep both seams.

## 5. Product decisions, kept short

| Topic | What the code does today | Decision |
|---|---|---|
| Open ownership | Only the SQL clause grants it (annotations.rs:442). `role_of` does not (store.rs:254-285). No production path creates an open document. | Query production for `open` rows once. Zero rows means delete the clause and the column. |
| Prometheus exporter | `/metrics` on the `[metrics]` listener (metrics.rs:512, serve.rs:433). docs/host/advanced.md:278 tells operators to scrape it. | Operator decision. Update that page if it goes. |
| Share update and replace | Exercised by agent_token_http_tests.rs:221, gated on a Postgres URL. The web client exposes create and revoke. | Product decision, with the test migrated if removed. |
| MCP independent batch | operations.rs:9-38 and 490-562, recovery at 1341, schema at tools.json:290-295. | Product decision. |
| Standalone companion lifecycle | Hidden `local start/stop/status/endpoint/diagnostics` (companion cli.rs:39-60, hidden at crates/librepaper/src/cli/mod.rs:107-112). HMAC challenge produced at service/mod.rs:1117-1146 and verified only by desktop/src/service.rs:196-244. Pairing and CORS are already gone. | Owner decision. |
| Native folder dialogs | Still in the companion (folder.rs:69-85, rfd on macOS and Windows, ashpd on Linux). The shell has message and save dialogs only. | Owner decision. Titles name origin and project, not the document. |
| Raw GitHub bearer | server/mod.rs:1126-1188 with `check_token` (github.rs:148-171). | Inventory who sends it before removing. |
| `/project` and `/snapshot` | Reader-safe allowlisted projection with ETag (documents.rs:1152) and Editor-only snapshot with the first comment page (documents.rs:1236, 1255). | Keep both. P4 fixes the cache header and the reader-link export. |

Not decisions. Account backup has no public CLI and lives in the companion routes (backup.rs:77, service/backups.rs:1131). `admin backup` is the operator export the drill uses (tools/test/backup/drill.sh:112, 165). Both stay. The benchmarks are all `#[ignore]` and nothing in CI runs them. Seeding runs only through `demo.simulate_activity_days` (onboarding.rs:167-181). Keep or drop them as maintainers please.

## 6. Order of work and ownership

The first batch has disjoint files and can run in parallel, one worktree each. P0 touches engine_adapter.rs and the discovery.rs comment. P2 touches store.rs and agent_view.rs. P3 touches sequencer.rs and log/tests.rs. P4 splits by file (cost.rs or documents.rs, builders/mod.rs, backfill.rs, export.rs or its help). P1 splits by crate. The central agent reviews every diff, then runs the checks once on the combined tree.

P5, P6 and P7 share server/mod.rs and the Postgres call sites and run one after another. P8 runs after the owner answer. P9 items go one at a time after the first batch.

## 7. Verification

- Per crate: `cargo check -p <crate>` and `cargo nextest run -p <crate>`. Before handoff: `make test`.
- Authorization and persistence: `tools/dev/db test`. After any SQL change: `tools/dev/db sqlx-prepare` then `tools/dev/db sqlx-check`. `make test` excludes the gated suite.
- Browser flows touched by P4 or P6: the relevant web/tests/integration scripts and `tools/test/suite browser`. `make test` excludes that suite too.
- Companion recovery (P8): current and legacy records, completed output, interrupted jobs, repeated recovery.
- Desktop: `cargo check --manifest-path desktop/Cargo.toml` and the platform checks for anything touched. `make test` does not cover it.

Nothing was built or run for this document.
