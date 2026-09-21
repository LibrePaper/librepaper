# Rust simplification review

Review date: 2026-09-21, working tree including the uncommitted diff. Read-only;
nothing here has been implemented. Companion to REVIEW-small-ideas.md (thirteen
local findings, several of which the current diff is landing) and
REVIEW-BIG-IDEAS.md (product-level). Nothing below repeats those. Line numbers
will drift; every reference was read in the working tree at review time. All
paths are under `crates/librepaper/src/` unless stated.

## Diagnosis

The sprawl has four root causes, and most findings are instances of one of them.

1. **Every transport re-derives the same things by hand.** HTTP, the room
   socket, MCP and the local bridge each spell out the per-request prologue
   (slug, origin, entry, viewer, role, room), the error-to-status mapping and
   the JSON envelope. There are 452 `write_json` calls and about 960 `json!`
   sites; the typed error `WriteError` exists but the transports mostly bypass
   it.
2. **Two models where one should be.** The local service keeps the v1
   `JobRequest` as its internal record and converts v2 into it; room code has a
   second reader of the document schema beside `Projected`; `DocumentService`
   sits inside `Server` for a migration that never happened; `history::Label`
   duplicates `TakeLabel`.
3. **The public surface defeats the dead-code lint.** `lib.rs` re-exports whole
   modules (`postgres`, `session`, `quarto`, `results`, `log`), so about twenty
   catalogue methods, a 270-line admission block, a 325-line TeX log parser and
   assorted helpers are dead without clippy noticing.
4. **Small helpers copied instead of shared.** SHA-256 hex has nine named
   copies plus 38 inline ones; "now" has eight clocks; atomic private-file
   write has six; constant-time compare three; process-group kill two; PATH
   lookup and version probe three.

## Live defects found on the way

Fix these before or alongside the refactors; each is small.

- **MCP reject and author refine can never succeed.** `Comment.proposed` and
  `Comment.outcome` are documented as "filled in when served" but the only
  production write is `proposed: None, outcome: String::new()`
  (room/comments.rs:400). `server/mcp/comments.rs:41-62` gates reject on
  `proposed.is_some()` and non-editor refine on the same, so both refuse.
  Drop the two fields and the two predicates.
- **Seven sites leak storage context to clients.** `plain(503,
  &error.to_string())` at documents.rs:1019, 1146 and history.rs:400, 482,
  630, 700, 794 sends `Display`, which for `WriteError::Storage` is "storage
  failure: {context}"; `client_message()` exists to withhold exactly that.
  Route through `refused("open room", &error)`.
- **HTTP comment writes always answer 400.** documents.rs:339-344 reads
  `result["status"]` from `apply_from`'s error value, which nothing sets, so a
  retryable storage failure and a 409 both become 400.
- **Socket reauthorization has two copies that disagree.** Only the bulk
  `reauthorize` (socket.rs:1461) flushes `AuthorityRevoked`; the per-frame
  `reauthorize_connection` (socket.rs:1370) does not.
- **Two full manifest inventories computed and discarded.** local/quarto.rs:954
  `_execution_inventory_before` and :1570 `_tracked_file_count` read and hash
  every manifest file, up to 64 MiB, for nothing.
- **Journal writer has a permission window.** assistant/journal.rs:260 writes
  the temp file then chmods 0600; every other writer opens with mode 0600.
- **Preview/job exclusion is asymmetric.** local/service.rs:2880 only looks at
  quarto jobs, so a pending native build never blocks a preview on the same
  root, while :2249 blocks any job.
- **`begin_account_erasure` has no completion path.** `finish_account_erasure`
  has no production caller and the worker's task table has no erasure task
  (repository.rs:144-201, worker.rs:8-13). Either dead or missing.
- **`document_policy` tests a CSP the server does not serve.** routes.rs:1185
  is `#[cfg(test)]`; production `serve_shell` and `serve_viewer` admit
  `https:` in `script-src`.

## Big architectural

### A. One request prologue and one refusal type for the server

The sequence valid_slug, cross_site_refused, checked_entry, viewer, auth_failed,
may_read, role gate, rooms.get is hand-written at 20+ sites (documents.rs
140-170, 228-262, 995-1015, 1041-1060, 1120-1140, 1584-1600; chat/mod.rs
48-64, 253-275, 305-330; mcp.rs 233-246; mcp/render.rs 91-105, 122-141;
suggestions.rs 51-84, 95-108; sharing.rs 238-265, 595-605; history.rs 384-400,
464-482; figures.rs 19-40, 66-80, 122-137; socket.rs 212-237;
quarto_checkpoint.rs 21-33; routes.rs 697-712). `entry_viewer` covers 13 of
them and stops before `may_read`. The refusal shapes have drifted: "bad slug"
is `plain(400)` in some routes and `write_json(400)` in others (24 sites),
`rooms.get` failure is mapped four ways.

Identity is also resolved two to four times per request. `cost::middleware`
computes `authenticated_identity` and stores it in `RequestContext`
(cost.rs:269, mod.rs:522), but no handler reads it; `Server::viewer` runs
`whoami` again (a Postgres read per cookie session), `publisher` a third time,
and an MCP call runs `mcp_recheck` two to four more times. Consequently
`Viewer.auth_failed` is provably false in every HTTP handler (the guard already
answered 401/503 for the same `Err`), so the 28 `if who.auth_failed` sites are
dead outside the socket reauthorizer.

Shape: `Server::open(&self, ctx: &RequestContext, slug, Access::{Read,
Comment, Edit, Owner}, need_room) -> Result<Opened { entry, who, room },
Refusal>`. `Refusal { status, message, retryable, fields }` in reply.rs with
`From<WriteError>`, `From<CommandError>`, `From<postgres::Error>` and
`IntoResponse`; handlers return `Result<Reply, Refusal>` and use `?`. Give
`CommandError` the same `status/client_message/is_temporary/log_context`
methods `WriteError` already has (room/error.rs:116-190) so the three
CommandError mappers (mod.rs:1631, mod.rs:1690, mcp/comments.rs:105, plus the
inline closure at socket.rs:1024) and three WriteError mappers (reply.rs:409,
reply.rs:437, mod.rs:1657) become one per transport. `AuthenticationFailure`
gets a `reply()` so its 401/503 mapping stops being written five times
(routes.rs:85, routes.rs:620, mod.rs:915, sharing.rs:565, quota.rs:27).

Size: 400-500 lines out of server/, one catalogue round trip per request, the
seven leak sites closed. This is the single highest-value change.

### B. Finish the route migration and delete `DocumentService`

routes.rs:100-495 is 400 lines of axum wrappers that differ only in which of
(request | headers, query, peer) the handler takes; routes.rs:640-830 still
holds three API routes inline in `dispatch` under "not yet expressed in
api_router", including the largest handler in the file. `DocumentService` plus
`Deref for Server` (mod.rs:86-131, 279-285) exists "while operations migrate
to its authorization-aware API", which is two methods and nothing migrated.

Shape: one handler signature `async fn handle_x(&self, ctx: &RequestContext,
request: Request<Body>, path) -> Result<Reply, Refusal>`, registered by one
generic closure; move the three routes in; fold `DocumentService` into
`Server`. About 450 lines.

### C. One request model in the local service

`PROTOCOL_VERSIONS = &[2]` (local/protocol.rs:15) and the browser only sends
v2, yet the service keeps v1 `JobRequest` (protocol.rs:277-310, every v2 field
as an `Option`) as its internal record and converts `BuildRequestV2` into it
at service.rs:1951-2088 (140 lines), then re-validates the same fields against
the `Option` mirrors at :2122-2148, re-checks scope at :2093 and :2227,
manifest at :2101 and :2239, source identity at protocol.rs:421 and
service.rs:2104. Previews do the same through `decode_preview`
(protocol.rs:551-620). Dead as a result: the `protocol != 2` branches
(service.rs:2076-2088, :2242-2246, :2823-2857, protocol.rs:552-554),
`engine_adapter::select`/`JobAdapter` and its v1 fallback, and the TeX-era
fields `engine/main/stem/max_passes/passes/incompatible`.

Shape: `BuildRequest` (the v2 shape) is the record in `JobEntry`, with
`builder: BuilderId` and a typed `enum BuildInputs { Quarto(QuartoJobOptions),
Native { options } }` derived once at admission; `PreviewRequest` carries
`enum PreviewBuilder { Quarto(..), Calepin(..) }`. Delete `JobRequest`. Also
type the 44 string-literal `status`/`stage` sites as `enum JobState` (the
"optional later refactor" of small-ideas finding 1) and collapse the four
identical terminal-outcome constructors (engine_adapter.rs:134,
builders/runner.rs:267, quarto.rs:1492, quarto.rs:1507). About 350 lines.

### D. One binding resolver, one preview prologue, one tool prober

Binding resolution (resolve scoped binding, canonicalize root and compare to
stored, hosted-or-entrypoint, manifest contains entrypoint, main under root)
is written at engine_adapter.rs:62-92, quarto.rs:727-850 plus :978-1030 and
:1080-1090, preview/quarto.rs:112-152, preview/calepin.rs:80-112,
service.rs:2158-2185; only the quarto job path rechecks the preset
`semantic_revision` before spawn. The two preview adapters share 45
line-for-line lines of admission (preview/quarto.rs:100-152,
preview/calepin.rs:69-112). Preset resolution is duplicated between
builders/runner.rs:100-160 and quarto.rs:741-790. PATH lookup and `--version`
probing exist in quarto.rs:650-712, preview/calepin.rs:148-263 (verbatim copy,
including `read_bounded`/`join_streams`) and discovery.rs:197-250 (the only one
using `run_confined_logged`); calepin is probed twice and its cached path
ignored by `calepin_plan`.

Shape: `BindingStore::resolve_for(..) -> ResolvedBinding` with
`still_valid()`; `Previews::start` does the generic admission and adapters
take `(&ResolvedBinding, options)`; `presets::Resolved::for_request` plus
`apply(&mut Command)` and `recheck`; `discovery::find(name, env)` and
`discovery::probe(path, args)` on `run_confined_logged`. About 300 lines, and
the security check lives once.

### E. Delete the TeX layer

`native.rs:9`, `engine_adapter.rs:7` and `builders/mod.rs:229` all state TeX
builds in the browser, and `BuilderId::parse` refuses `tex|latexmk|tectonic`.
Still present: `validate_shape` accepting those builders (protocol.rs:465, a
job that always fails at runner.rs:82); `texlog.rs` (325 lines) reachable only
from a dead branch in builders/diagnostics.rs:9; tex branches at
runner.rs:173, :241; TEXMF env exports at runner.rs:20-56; discovery.rs TeX
search dirs (109-166), makeindex/latexmk/biber banner parsing (218-361),
`kpsewhich` (366-383) and `Cache.bin_dir/texmf_root/distribution`;
`Distribution`, `ToolVersions`, `JobStatus.passes/incompatible`,
`JobOptions.max_passes`, `MAX_PDF_BYTES` in protocol.rs; `--tex-path` /
`LIBREPAPER_TEX_PATH`. Also the "retained for wire compatibility" fields
`Capabilities.tools` and `Confinement` (always `none`), read by two Svelte
lines. About 600 lines, pure deletion under the hard-cutover rule.

### F. `Projected` is the one read model of a document

`session::{texts_of, paths_of, text_ids_of, assets_of}` (document/session) is
a second reader of the Loro schema beside `librepaper_document_core::project`.
It applies no path rules, so on a path collision it silently overwrites where
`project()` suffixes `name (2).ext`; `locate_anchor`, `Sources::of`
(resolve.rs:106) and `Store::project_files` (store.rs:832) therefore see a
different file set than every reader, exporter and agent. Separately
`Room::attach` and `reattach_comments` (resolve.rs:451, 520) run the full
projection (walk and SHA-256 every text) inside `with_head` on every comment
page read purely to get the digest that `Sequencer::projection()` already
caches. session/shape.rs:37-44 redeclares the root constants core exports.

Shape: `locate_anchor(&Projected, ..)`, `Sources::of(&Projected)`, digest from
`self.log().projection()`; session keeps only mutators and imports constants
from core. About 120 lines plus the per-page recompute.

The core crate itself is not consumed by any wasm build (web/ has a hand-written
JS port kept equal by a fixture); either fold it into `document/` or make
`session` import from it, but stop letting "shared with wasm" steer design.

### G. Comment writes: one caller context, typed events, no re-reads

The caller context (creator, authority, authorization, author id, author key)
is built identically at server/mod.rs:1208-1226 and mcp/comments.rs:167-175,
256-260 and passed positionally to eight command constructors (AddComment
takes 17 arguments). After a command returns the `Comment` it wrote, the server
serialises it, `broadcast_comment_event` string-matches `payload["type"]` and
re-reads the comment (three queries) plus `comment_state` (one) once per role,
and the socket or HTTP caller reads it a third time: up to nine reads to
rebuild a value in hand. Author name and key are computed in nine places
(mod.rs:1206, socket.rs:591, mcp/comments.rs:172, routes.rs:770,
suggestions.rs:146, mcp/operations.rs:829, `comment_author`, socket.rs:240,
`mcp_author`). Resolve/Accept/Reject/Refine each read the row, rebuild a
`NewAnnotation`, replace the whole row, then read it again to flip one
boolean.

Shape: `CommentWriter` built once per request; `run_comment_command` returns
`enum CommentEvent` with `view(&self, viewer)`; `Viewer::creator_name(slug)`
computed once in `viewer()`; a catalogue `set_resolved_authorized(..)
RETURNING`. About 250 lines and six to nine round trips per mutation.

### H. Narrow the public surface so the lint can work

`lib.rs` re-exports `postgres`, `session`, `quarto`, `results` and `pub mod
log`. Integration tests use only `AccountRecord`, `PostgresCatalog`,
`PostgresOptions` and a few log types; nothing references
`librepaper::quarto` or `librepaper::results`. Verified dead behind that
surface: `documents_by_owner(_page)`, `check_storage_admission`,
`asset_sizes_by_digests`, `replace_suggestion_authorized`,
`supersede_proposals`, `forget_marks`, `complete_asset`,
`retained_asset_bytes`, `annotation_count`, `apply_annotation_batch` and its
two types, `label_archive_keys`, `access_role`, `set_grant`, `remove_grant`,
`create_share_link`, `revoke_share_link`, `Sequencer::{is_warm, recover,
drop_cache, subscribers}`, `quarto::classify_freshness`, the session helpers
`cursor_at_path`, `replace_text`, `apply_edits`, `main_id`, `has_meta`,
`mark_meta`, `latex_engine`, `text_of`, `Presence` (154 lines), and the whole
admission block sync.rs:64-336 (the sequencer imports directly since the log
cutover). `document/hunks.rs:45` carries a stale `#![allow(dead_code)]`
("waits for Phase 2"; proposals.rs:49 already consumes it).

Shape: re-export by name, make the rest `pub(crate)`, drop the allow, switch
remaining allows to `#[expect]`, put `[lints.clippy]` in Cargo.toml. About 900
lines fall out, and the lint stays honest afterwards.

## Medium

- **Sequencer preambles and fan-outs** (log/sequencer.rs). The readiness check
  (fenced, unreadable, build cache) is copied five times (1844, 2095, 2139,
  2167, 2387); the subscriber fan-out loop six times (1776, 2580, 2655, 2677,
  2710, 2740); row staging twice (1439-1477, 1894-1924); `close_fenced` and
  `CompactionGate::fence` are identical. `unreadable: Option<String>` plus
  `fenced: Option<String>` plus `cache`/`projection` `Option` pairs encode a
  state machine by hand. Shape: `enum Health`, `projection` inside `Cache`,
  `send_all(..)`, `Inner::stage(..)`. About 250 lines.
- **Writer-epoch fence spelled four times** (ownership.rs:58,
  document_log.rs:265, :317, :495), two with `singleton=true` and two with
  `singleton`. `begin_fenced_flush`/`flush_log_row` are test-only twins of the
  production charged path. `seed/activity.rs:531` writes log rows directly,
  bypassing the sequencer and its pending budget; decide whether that is
  intended and say so.
- **One storage error.** `collaboration::Error` and `source::Error` differ by
  one variant, have one caller each, and both callers `.to_string()`. Ten error
  types and thirteen `From` impls in storage/ and log/; worker.rs has 20
  `map_err(|e| e.to_string())`. Shape: `storage::Error { Database, Blob,
  Archive, Io, Invalid }`, wrapped once by the sequencer errors. Do not add
  thiserror for this; the manual impls are short.
- **`budget.rs` and `pending.rs` are one bounded-counter primitive** written
  twice (compare-exchange loop, `Notify` wait, `Reservation` with `Drop`,
  `snapshot()` JSON). Shape: `log::pool::Pool` with `Lease`. About 90 lines.
- **`_in_transaction` twins in annotations.rs** (1056/1073/1089, 896/916,
  832/809, 1008/962) where `repository.rs:1101` already shows the
  executor-generic shape; two SQL conventions in one directory (`const
  COLUMNS` + runtime `query_as` vs checked macros); the keyset clause spelled
  five times. Raw SQL outside the catalogue at worker.rs:451 (duplicating
  document_log.rs:645) and maintenance.rs:29, :44, :158.
- **`LogCatalog` trait**: 11 methods, fake implements 6; `log_head`/`log_base`
  exist only for `admit`, which holds the concrete type; hand-written
  `BoxFuture` while `BlobStore` uses `async_trait`.
- **Two label commands.** `history::Label` (history.rs:100-160) and
  `TakeLabel` (room/label.rs:77-150) both write a `NewLabel` naming head;
  `Label` decodes the digest by hand and has no `replay`. Keep `TakeLabel`.
- **`client_seq_of` four times with two semantics** (proposals.rs:302,
  store.rs:493, history.rs:92 use bytes 0..8; agent.rs:534 uses 8..16;
  comments.rs:1888 a fifth inline). `Head::prepare` should derive it from the
  `Uuid`. `fresh_peer` duplicated verbatim; `label_for_request` (agent.rs:667)
  duplicates `PostgresCatalog::label_by_request`.
- **Newtypes worth having**: `TreeDigest([u8; 32])` (hex decoded by hand at
  seven sites; `Evidence` carries hex while `NewLabel` carries bytes),
  `Frontier` (16 `Frontiers::decode` sites, hand serde in annotation.rs:51),
  `Principal { Account, Link, Visitor, Agent }` (nine `format!` sites, four
  prefixes, one constant), and `comment_id: Uuid` through `RoomCommand`
  (parsed in every arm of `run_comment_command`, mod.rs:1335-1484). Document
  and account ids are already `Uuid` almost everywhere; do not newtype those.
- **`document/store.rs` vestiges**: `get_checked` aliases `get_result`;
  `open_with_catalog` cannot fail; `owned_by(_owner_key, ..)` ignores its
  argument yet nine call sites thread it; `IndexEntry.size` always 0,
  `bookmark_link_hash` always `None`, `sha` is `update_sequence.to_string()`;
  `PutError`/`ModifyError` hand-roll a status table `WriteError::status()`
  already is.
- **`room/agent.rs` validates by cloning the whole tree** (`apply_patches`
  at 255-350) and discards the result (594-606); MCP repeats the same under
  the lock. `AgentAuthority` to `MutationActor` to `MutationAuthorization` to
  `Authority` in one function. Shape: `validate_patches(&tree, &req) ->
  Result<usize>`.
- **Four principal shapes in server/**: `Viewer`, `Caller` (an `Identity`
  flattened to five strings and rebuilt), `MutationActor` (hand-built six
  times with `policy_editor` sometimes literally `true`), `Authority`. Shape:
  `Viewer::mutation_actor(policy_editor)`.
- **Two socket pump loops** (`run_socket` socket.rs:312-1335,
  `run_chat_socket` chat/mod.rs:98-238) each implement write-timeout draining,
  Close, ping, link-expiry, connection registry and budget admission.
- **`service.rs` split** (local, 3.2k lines): http plumbing (~350), job table
  and queue (~1300), pairing (~450), bindings, agents/zotero, previews,
  capabilities. `management.rs` and `assistant.rs` already show the
  `&Inner` + `pub(super)` pattern. The local service also duplicates
  `write_json`, `plain`, `set`, `header_str` from server/reply.rs and
  hand-matches path segments inside an axum fallback instead of routing.
- **Agent packages**: `assistant/` and `automation/` import each other by
  intent; fold into one `agent/` package; move `agent_query.rs` under `room/`
  beside `agent.rs` and `agent_view.rs`; `QuerySnapshot` is built in three
  places. Three unrelated `Task` types, two `Admission`, two `Capabilities`,
  three `Reservation`, two `Retry`, two `Role`.
- **Quarto in three modules by history**: top-level `quarto/` (parser,
  fingerprint) is only used by local/; `local/quarto.rs` is bindings +
  execution + collection in one 3.5k file with `#[cfg(test)]` production-shaped
  functions (`import_artifact`, 105 lines) kept for `quarto_tests.rs`; two
  `computation_fingerprint`s. Shape: `local/quarto/{parse,bindings,run,collect}.rs`.
- **Test fixtures**: `struct Deployment` + `deployment()` is defined in eight
  files, the 12-table `TRUNCATE` literal appears 20 times in seven files, three
  `catalog()` helpers, and `tests/` duplicates `blobs/connected/now_unix/
  bearer_token/truncate` between its two Postgres files. `log/recovery.rs`
  (2k) is a test file with a production name; `postgres/mod.rs` is 283 lines
  of code and 1,858 of inline tests. Shape: `src/tests/support.rs` with
  `fresh_catalog()`, `TRUNCATE_ALL`, `DeploymentBuilder`; `tests/common/`.
  About 500 lines and the table list stops drifting.
- **Config**: `extensions` and `max_replies` are set and never read; the
  advanced YAML is parsed twice (cli/mod.rs:263, :283); ten runtime
  `LIBREPAPER_*` knobs bypass clap and never appear in `--help`; the
  chat/document/conversation triple is read in four modules (17 sites).
- **CLI export** mirrors the comment page wire shape by hand (export.rs:555)
  against a `json!` on the server and a differently named in-memory
  `CommentPage`; `Walk` uses raw headers instead of `get_as`.

## Small and mechanical

Each under an hour; do them when touching the file, or in one sweep.

- `util::sha256_hex`: replaces `digest_of_bytes`, `results::sha256`,
  `quarto::sha256`, two `hex_sha256`, `agent_query::digest`,
  `device::digest_of`, `pairing::hash_token`, and 38 inline
  `hex::encode(Sha256::digest(..))`.
- One clock: `auth::now_unix` and `quarto::now_unix` duplicate
  `util::now_unix`; `quarto::timestamp` emits fractional seconds where util is
  to-the-second; `quarto::timestamp_from(_started)` ignores its argument so
  every bundle has `started_at == completed_at`; `chat/hub::now`,
  `backup::now`, `lifecycle::unix_now`, `journal::now` (assistant/ is the only
  `u64` user).
- One private-file writer: keep `private_files::publish`, add a create-only
  mode for the auth key, delete `cli/tokens::write_private_file`,
  `backup::write_private_file` (same name, not atomic),
  `journal::publish_durable_private`, the inline `presets::save`, and the
  tmp+rename loops in `BindingStore::save`, `discovery::save_cache`,
  `persist_quarto_job`, `write_hosted_*`, `persist_frozen_cache_identity`.
- One constant-time compare (`pairing.rs:258`, `service.rs:1460`,
  `service.rs:1684`).
- One process-group kill: move `quarto::terminate_process_group` (spawns a
  `kill` binary) next to `native::kill_tree`.
- `local/quarto.rs:2750` hand-implements MD5 (70 lines); `md-5` is already in
  Cargo.lock.
- `engine_adapter::quarto_shared_paths` is `#[allow(dead_code)]` with no
  callers (85 lines).
- `common_prefix/suffix` twice (locate.rs:267 over char, resolve.rs:359 over
  u16); `is_html` three times; `by_words` reimplements `Search::find` scoring
  without the `DECISIVE` margin, so "ambiguous" means two different things;
  `Search.files` is dead (`let _ = self.files;`); `AnchorStatus::parse` and
  `ResolutionDiagnostic::parse` have no callers.
- `decode_update` validates by importing into a scratch doc, then
  `apply_update` imports again; `text_at/string_at/id_of_path` defined three
  times; `agent.rs:524` calls `apply_path_edits` (exports a diff per file) and
  discards it where `apply_edits_at` is meant.
- `query_map` and `json_body<T>(request, limit)` helpers: the form-urlencoded
  collect is written 11 times, `to_bytes` + `from_slice` 14 times with eight
  different literal limits and four error shapes.
- `.max(0) as u64` on columns with `CHECK (>= 0)` at seven sites; three
  identical `note_compacted` marshalling blocks in worker.rs.
- `document_log.rs:420` bounds a row by `DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES`,
  a fourth spelling of the row bound the sequencer comment says must agree.
- `FsStore` (blob.rs:299) is a 46-line delegation wrapper for one caller;
  `BlobStore::delete` loops per key while worker.rs says it batches.
- `POST /api/documents/{slug}/opened` has no caller; `upsert_account_job`
  forwards; underscore-kept unused params at mod.rs:1168, suggestions.rs:47,
  mcp/render.rs:413, mcp/comments.rs:132, :135, mcp/operations.rs:455, :770.
- `Room::take_label` vs `take_label_reporting_replay`, `command` vs
  `command_reporting_replay`: return `(output, replayed)` once.
- `room/mod.rs:726-738` one-line wrappers over `document::render`.
- Three state-home resolvers, two loopback health probes, `truncate_utf8`
  duplicating `http::truncate` (which belongs in util).

## What to leave alone

- `Result<_, String>` in cli/, local/ and assistant/ (296 signatures). Those
  errors terminate in `die` or a JSON message to the browser; a typed error
  there adds code without a consumer. The convention worth changing is the
  server's, where `WriteError` exists and is bypassed.
- The sequencer command boundary, `Projected`, and the engine-neutral preview
  `Watch`/`Plan` design. They are the foundations the findings above build on.
- `assistant/task.rs` keeping `serde_json::Value`: its fingerprints are
  persisted (small-ideas finding 12).
- `Uuid` for document and account ids: already the norm; a newtype buys little.
- Not adopting anyhow or thiserror as a blanket rule.

## Suggested order

1. The live defects, each a small targeted change.
2. H (narrow the public surface) first, because it makes the lint report the
   rest for free, then E (TeX cull), both mostly deletion.
3. A and B together (server prologue, refusal type, route migration).
4. C, then D (local request model, then binding and probe consolidation),
   then the `service.rs` split, which moves less once C and D have landed.
5. F and G with the newtypes (`TreeDigest`, `Frontier`, `Principal`, comment
   id) done first, since both are easier typed.
6. The sequencer helpers, one storage error, the pool primitive, the test
   fixture module.
7. The small sweep: sha256, clocks, private writes, constant-time compare.
