# Delete the assistant runner process

## 1. Current shape

"Start assistant" in the sidebar produces four processes tied to one
(connection, conversation) pair.

**Companion.** `librepaper local start` serves `POST /assistant` in
`crates/librepaper/src/local/assistant.rs` (`handle_assistant_start`,
lines 13 to 125). It resolves the agent command, computes an expected
`config_hash` from the document link and the command
(`crate::assistant::lifecycle::configuration_hash`), and calls
`crate::assistant::start` (`crates/librepaper/src/assistant/mod.rs`
lines 17 to 28) inside `spawn_blocking`. `start` calls
`lifecycle::start_or_replace`, which is idempotent: a running runner with
the same `config_hash` is left alone; a different one is stopped first
through the nonce-bound control file, then replaced.

**Runner.** `start_background` (`assistant/lifecycle.rs`, lines 291 to
400) spawns the current executable as `librepaper agent connect -
<conversation> --agent ... --state-directory ...`, in its own process
group, with `LIBREPAPER_DOCUMENT` (the credential URL, key included)
and `LIBREPAPER_CHAT_TOKEN` as environment variables (lines 322 to
324). Stderr goes to `runner.log`, read back on failure by
`last_log_line` (lines 262 to 268). This is the only place that spawns
`agent connect`. `cli/agent.rs` (`AgentCommand::Connect`, lines 15 to
49) is the hidden CLI surface; with `--background` it calls
`start_background` itself and prints a JSON line, otherwise it calls
`crate::assistant::runtime::run` in the foreground.

**Model turn loop.** `runtime.rs::run` (lines 456 to 480) acquires a
`Lease` (a flock plus a nonce, in `lifecycle.rs`), then `execute`
(lines 481 to 946) drives one ACP agent process across the runner's
lifetime: dispatch queued tasks, stream answers, handle permission
requests, cancel on timeout, and reconcile with the browser's chat
`Transport` (a separate websocket connection to the document's chat
channel). The ACP child is started by `start_agent_with_bridge` (lines
192 to 261), which calls `acp::Agent::start` (`assistant/acp.rs`, lines
223 to 284): it configures the subprocess's environment, issues
`session/new` (never `session/load`, comment at runtime.rs lines 526 to
536), and hands the agent an MCP server descriptor for `librepaper
agent mcp --connection <name>` as its own document tool.

**MCP bridge.** The third party agent spawns `librepaper agent mcp
--connection NAME` itself (`cli/agent.rs` lines 76 to 93), resolving the
connection through `local::connections::ConnectionStore::resolve`, then
serving `automation::mcp::stdio` over stdio.

**Secrets and their hops.** The document key travels: browser to
companion (the `POST /assistant` body carries the connection name, not
the key) to runner (`LIBREPAPER_DOCUMENT` env var, credential URL) to a
synthesized internal connection (`internal_connection`, runtime.rs
lines 66 to 83, writing a `runner-<hash>` entry into `connections.json`
via `put_internal`) that the MCP subprocess resolves by name, never by
env var. The chat token follows the same path via `LIBREPAPER_CHAT_TOKEN`.
Neither reaches the ACP agent's own process arguments or environment
(test `the_adapter_environment_carries_paths_and_no_credential`,
runtime.rs lines 1072 to 1091); the agent only sees journal and epoch
file paths (`adapter_environment`, lines 47 to 64).

**Status.** Three representations, not one. The runner's own view is
`runner.status.json` (`Status` struct, lifecycle.rs lines 56 to 70:
pid, nonce, state, config_hash, detail), read by `lifecycle::status`
with a liveness check against the flock (lines 447 to 464). The
per-task view is the journal (`assistant/journal.rs`): a bounded,
receipt-only log of operation identities and outcomes, replayed to the
browser on reconnect by `reconcile` (runtime.rs lines 399 to 418). The
companion's own record of "is this configuration still what's running"
is the `nonce` and `config_hash` written into `connections.json` by
`set_assistant_configuration` (`local/connections.rs` lines 303 to 328),
compared against the live runner status in `handle_assistant_start`
(lines 83 to 119, on start) and `handle_assistant_status` (lines 156
to 166, on poll). These are three independent representations that the
companion reconciles by reading all three and rejecting on mismatch,
not one mechanism crossing process boundaries.

## 2. Target shape

The companion drives ACP directly: one supervised tokio task per
(connection, conversation), owning the `Lease`, the `Journal`, the
`Transport`, and the ACP child, inside the companion's own process.
`execute` in `runtime.rs` becomes that task's body largely unchanged;
`run` becomes the task's entry point, called directly instead of via a
spawned binary.

Deleted: the whole `AgentCommand::Connect` variant and its
`--background` flag (`cli/agent.rs`); `lifecycle::start_background` and
`wait_until_free` as a cross-process protocol (the lock file and nonce
become in-process bookkeeping, not an IPC surface); `LIBREPAPER_DOCUMENT`,
`LIBREPAPER_CHAT_TOKEN`, `LIBREPAPER_ASSISTANT_STATE_DIR` as env vars
(they become plain function arguments); `runner.log` and
`last_log_line` (a task's failure is a `Result` in-process, not stderr
from a child); the `runner.lock` flock as the liveness proof (replaced
by the companion's own task registry, which cannot lie about whether a
task handle is live). `runner.status.json` likely survives in reduced
form, since external diagnosis is still worth keeping, but it stops
being the liveness proof and becomes only a diagnostic mirror.

## 3. What must survive

**Crash isolation.** One wedged or panicking ACP session must not take
down builds, previews, and pairing served by the same companion. Each
session is its own tokio task, holding its own `Lease` and its own
`acp::Agent` (which owns the child process handle). A panic inside a
task is caught at the task boundary: `tokio::spawn` already turns a
panic into a `JoinError` for the caller, provided the workspace does
not set `panic = "abort"` (it does not today), so this holds without
extra `catch_unwind`. A wedged agent is stopped by killing its child
process directly, not by waiting on a cooperative exit; `acp::Agent`
needs a hard-kill path beyond `shutdown()`, since its `Drop` today only
aborts the tokio task (acp.rs lines 215 to 219), which does not
guarantee the child dies. Channels between the companion's request
handlers and each session task must stay bounded, as `Transport` and
the ACP event channel already are (`mpsc::channel(16)` and `(32)` in
acp.rs), so one stuck session cannot exhaust memory.

**Journal recovery after a companion restart.** `recover_state`
(runtime.rs lines 433 to 454) marks `interrupted` any task that was
`working` when the process last stopped, and attaches whatever receipts
the journal already has. This must still run once per session at the
moment the companion (not a runner) restarts and picks the session back
up, and the semantics (never silently replay a mutation; unresolved
effects stay visible as `interrupted`) do not change.

**Never `session/load`.** The comment at runtime.rs lines 526 to 536
explains why: a resumed session keeps the MCP servers it was created
with, not the ones handed to `session/load`, so resuming an agent
produced one with no document tools. This constraint has nothing to do
with process boundaries and must be preserved verbatim: every session
task always calls `session/new`.

**Peer headers and the epoch file.** `mcp_request_with_epoch`
(`crates/librepaper/src/automation/peer.rs`, lines 464 to 534) attaches
`x-librepaper-runner-conversation` and `x-librepaper-execution-epoch`
headers, deriving the epoch from
`crate::assistant::journal::execution_epoch` at a path named by an env
var today. Server-side, `document_result` and the operation-refusal
path (`server/mcp/operations.rs` lines 373 to 410, 440) use this epoch
to attribute a durable receipt to the runner's current execution rather
than to the agent's raw MCP call. The mechanism itself (a per-execution
epoch file swapped on every restart, guarded so a superseded execution
cannot mint a receipt after a newer one has taken over) must move
into the companion process unchanged in meaning; only the transport
between the companion and the MCP subprocess (currently env vars,
`adapter_environment`, runtime.rs lines 47 to 64) needs to keep
resolving to a real file path for `librepaper agent mcp` to read, since
that binary still runs as a separate process (see section 4).

**Receipt reconciliation.** This is the same epoch mechanism from the
document's perspective: a browser or any other MCP client can call
`document_result` today, but only a runner-attributed call can prove an
operation was admitted under a specific execution. This survives by
construction if the epoch file and headers survive.

## 4. Why `agent mcp` stays a separate process

The third party coding agent (Claude Code, Gemini, or whatever ACP
implementation the user picked) spawns its own MCP server according to
the `McpServer::Stdio` descriptor the companion hands it at `session/new`
(acp.rs lines 267 to 283). That spawn call belongs to the agent, not to
LibrePaper; deleting the runner does not change who owns the fork.
`librepaper agent mcp --connection NAME` must keep working exactly as a
standalone binary invocation, because nothing else can start it.

## 5. Migration steps

Each step should be small enough to land and run the existing suite on
its own.

1. Extract `execute`'s setup and per-tick body into functions taking
   an already-open `Lease`, `Journal`, and `Transport` by reference,
   with no dependency on being the process's `main`. Test: existing
   `runtime` unit tests should compile and pass unchanged, since they
   already call `persist_report`, `recover_state`, and friends
   directly.
2. Add a companion-side session registry (task handle, `Lease` handle,
   cancellation) that spawns a session task calling the extracted
   `run`/`execute` in-process, and in the same change switch
   `handle_assistant_start`, `handle_assistant_status` and
   `handle_assistant_stop` to it. Delete the old
   `start_background`/`wait_until_free` process-spawn path,
   `AgentCommand::Connect`'s `--background` flag, and the
   `LIBREPAPER_DOCUMENT`/`LIBREPAPER_CHAT_TOKEN` env-var plumbing in
   that change too: the two paths never coexist. Test: keep every
   existing sidebar assistant integration test (start, status poll,
   config_hash mismatch on concurrent start, stop) passing against the
   new path; add a test for a wedged session being hard-killed without
   affecting a second concurrent session in the same companion.
3. Add a companion-restart recovery path that re-attaches
   `recover_state` semantics for any session directory left in a
   non-terminal state, run once at companion startup rather than once
   per runner process start. Test: kill the companion mid-turn in an
   integration test, restart it, and assert the task surfaces as
   `interrupted` with retained receipts, mirroring
   `restart_keeps_terminal_effects_after_journal_eviction`.
4. Remove `runner.log`, `last_log_line`, and the CLI's now-dead
   `Connect` variant; fold what remains of `Status`/`Lease` into
   companion-internal types once the on-disk shape no longer needs to
   satisfy a cross-process contract. Test: `cargo clippy -D warnings`
   catches anything still referencing the deleted path; keep the
   `agent mcp --connection` integration tests unchanged, since that
   binary's contract does not move.

## 6. Risks and open questions

Killing the ACP child hard (not cooperative `shutdown()`) needs
confirmation that the ACP crate exposes a real kill handle reachable
from `acp::Agent`, not just task abort; today's `Drop` only aborts the
tokio task (acp.rs lines 215 to 219), which may leave an orphaned child
if the underlying transport does not also die with its owning task.
Verify this against the `agent-client-protocol` crate before step 2
lands, rather than assuming it.

Moving many concurrent sessions into one companion process changes the
failure blast radius from "one runner dies" to "one bug in the shared
session registry stalls every session." The per-task isolation in
section 3 is necessary, but the registry itself is a new single point
of failure; it needs its own test for a session that panics during
registration, not only during its main loop.

The restart-recovery step (4) assumes the companion restarts rarely
enough that walking every session directory on startup is cheap. If
the companion restarts more often than a runner used to, that walk
needs a bound, which no current code establishes.

Whether `runner.status.json`, or something like it, should still exist
for external diagnosis is an open product question, not only an
implementation one: `librepaper agent status`-style tooling may be
relied on by testers or documentation today.
