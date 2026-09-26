# The assistant runner moved into the companion

This was a plan; it is now done. What follows is a short record of the
result, not a proposal.

## What changed

The companion (`librepaper local start`) no longer spawns `librepaper
run-agent` as a child process per (document, conversation) pair. Instead
`crate::assistant::registry::SessionRegistry`, a field of `local::service`'s
`Inner`, runs one supervised tokio task per pair inside the companion's own
process, calling `assistant::runtime::run`/`execute` directly. Starting,
polling and stopping the sidebar assistant (`POST /assistant`,
`/assistant/status`, `/assistant/stop`) now go through that registry instead
of through `assistant::lifecycle::start_or_replace`/`status`/`stop`, which
read and wrote a cross-process flock, a control file and `runner.status.json`.

Deleted: `assistant::lifecycle::start_background` and the 75-second polling
loop that used to watch a spawned child's status file and log; the hidden
`librepaper run-agent` command (`RunAgentArgs`, `run_agent`, and the
`Command::RunAgent` CLI variant); the `LIBREPAPER_DOCUMENT` and
`LIBREPAPER_CHAT_TOKEN` environment hop (the registry passes the link and
token as plain arguments); `runner.log` and `last_log_line`; the
`runner.lock` flock and `runner.status.json` as cross-process liveness proof
and status mirror. A session's coarse status now lives only in memory
(`lifecycle::StatusHandle`), owned by the registry entry for as long as the
task runs and for one status poll after it ends.

Kept unchanged in meaning: `librepaper mcp`, the document-tool MCP bridge the
third party agent still spawns as its own separate process, since that spawn
belongs to the agent, not to LibrePaper (`mcp --connection NAME` is now its
only accepted form; the positional-link and `LIBREPAPER_DOCUMENT`-via-`-`
forms are gone, since the registry is the only caller and it always names a
connection). The execution epoch file and the adapter environment
(`adapter_environment` in `runtime.rs`) that the MCP subprocess reads to
attribute a receipt to the runner's current execution. The rule that a
session always calls `session/new`, never `session/load`. The `binding_nonce`
file, since it is the one piece of session identity that has to survive a
companion restart.

## Hard kill

Stopping a session (`SessionRegistry::stop`) requests the cooperative
shutdown a healthy session's own tick loop would take anyway, then aborts its
tokio task outright and awaits the abort. That abort is the hard kill: per
`agent-client-protocol` 2.2.0 (`ChildGuard` in `src/acp_agent.rs`), dropping
the ACP connection future SIGKILLs the agent child's whole process group, and
aborting the task that owns that future is what drops it. There is no
separate PID or kill handle anywhere in this path.

## Recovery

At companion startup, before any assistant route is served,
`SessionRegistry::recover_at_startup` walks `<state_home>/librepaper/assistant`
and runs `runtime::recover_state` (unchanged: a task left `working` becomes
`interrupted`, with whatever journal receipts are still within the journal's
bound) against every session directory it finds, bounded to
`MAX_RECOVERED_SESSIONS` (500) directories so an accumulated `assistant/`
directory cannot turn a restart into an unbounded walk. No session task is
started during recovery; the sidebar starts a fresh one the normal way.

## Tests

`assistant::registry` has its own unit tests: two concurrent sessions where
hard-stopping one leaves the other running, a session whose task panics
reporting a failed status while the registry keeps serving a second session,
and a recovery test that a session directory left mid-task is surfaced with
its journal receipts attached after a simulated restart. They exercise the
registry with an injected async body rather than a real ACP agent or network
access, since no fake-agent test harness existed to reuse.
`tests/cli_surface.rs` was updated for the deleted `run-agent` command and
for `mcp` now requiring `--connection`.
