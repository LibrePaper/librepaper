# Resume the refactor

Stopped at the user's request on 2026-09-08. Resume all twelve tracks in
SPEC-refactor.md; do not mistake this checkpoint for completion. Evaluate
choices from first principles. The user authorized cheaper parallel agents,
with parent review before merging into `live-markdown-editor`.

## Integrated checkpoint

Main is `live-markdown-editor`, retention merge `c6e8a72` followed by this handoff.
Registry locks, rendering lookup/HEAD, typed command dispatch, shared retention,
and content-identity accessors are integrated. See STATUS.md for individual
commits and remaining scope. Retention passed full workspace tests (783 library
tests, one existing ignored test, all other suites passing), strict Clippy
across all targets/features, formatting and diff checks. Logs:
`/tmp/refactor-retention-workspace.log`, `/tmp/refactor-retention-clippy.log`.
Its tests cover 201 events over nine distinct trees, one paginated traversal,
four concurrent reads, failed reads preserving assets/text, and live edits
during text-object listing. Manifest ownership pins the retention graph.

## Committed branches awaiting review

Do not merge these solely on agent-reported green checks. All agents stopped.
Local git branches preserve the work independently of temporary worktrees.

| Branch | Tip | Worktree | State |
| --- | --- | --- | --- |
| `refactor/comment-locks` | `154c658` | `/tmp/librepaper-refactor-commands` | Parent review pending; agent reports 787 library tests passed, one ignored, Clippy/fmt clean. |
| `refactor/attribution` | `9078109` | `/tmp/librepaper-refactor-attribution` | Second review fixes pending parent verification; agent reports workspace and strict Clippy passed. |
| `refactor/size-limits` | `a120066` | `/tmp/librepaper-refactor-size` | First review fixes pending parent verification; agent reports workspace and strict Clippy passed. |
| `refactor/resident-estimates` | `f38cdff` | `/tmp/librepaper-refactor-memory` | Profiling test only, explicitly ignored in ordinary suites; baseline executed successfully. |

Comment-lock review: verify conditional snapshots, serialized operation gates,
detached reconciliation lifetime, quota/purge ownership and failure rollback.
Agent added paused-I/O, concurrent-write, cancellation and purge tests. Source
and sockets should proceed during legacy blob writes. Synchronous catalogue
paths remain a separate migration. This branch is based on `edf8d5f`.

Attribution commits: `81d53e8`, `b0471a7`, `9078109`. Adds nullable stable IDs
without guessing legacy handle matches. Earlier review found stale cache
completion, staged receipt copies, cold-load races, and incomplete abort
accounting. Latest agent fixes claim:

- Shared transactional abort refunds peak reservations exactly once and clears
  pending publication; test erases an editor on another owner's document.
- Opaque legacy non-JSON intents cannot stall erasure; unresolved opaque text
  remains an explicitly documented limitation.
- Structured receipt redaction markers preserve original-request retries while
  rejecting different nonredacted data and digest mismatches.
- Cold-load cache installation participates in the attribution RW gate;
  catalogue reads suppress erasing/missing-account attribution before physical
  SQL rewriting. Review lock order and barrier tests, including staged writes.

Size commits: `f130518`, `a120066`. Proposed defaults/policy: source 4 MiB default,
8 MiB maximum, encoded new snapshot 16 MiB, queued bytes 64 MiB, preparation
memory 512 MiB; old decoding limit remains 64 MiB. Earlier review found reusable
reservation tokens bypassed accounting and detached journal work outlived room
quota/gates. Latest fixes claim runtime-bound exclusive operation leases,
pre-spawn byte/record admission, and service-owned room/quota/writer/state/eviction
pins through reconciliation. Verify cancellation, eviction, durable withdrawal,
purge and retry tests. Recovery now drops old buffers and sorts references;
verify actual simultaneous allocation bounds, not just constants in a formula.
The 8 MiB source maximum is a supported policy, not a claim that larger text
can never encode below 16 MiB.

## Resident profiling and remaining work

Baseline: 200 unchanged estimates of 1,000 comments with 1,024-byte bodies and
64 checkpoints took **4.091964804 seconds** in debug; estimated resident bytes
were 1,207,342. Log `/tmp/refactor-resident-before.log`. Reproduce on the resident
branch with `cargo test -p komodoc --lib --offline
room::resident::tests::profile_unchanged_resident_metadata -- --ignored --nocapture`.
No cache implementation exists yet. Consider a transparent mutation-invalidated
wrapper for comments/manifest rather than fragile manual invalidation at dozens
of callers. Preserve wire serialization and snapshot clones; measure the same
workload afterward, test mutation invalidation, and use saturating estimates.

Next: review pending branches, integrate individually, resolve conflicts and
run combined tests. Then finish catalogue execution, resident estimates, typed
write errors, remaining shared helpers, S3 operations and backup ownership.
STATUS.md retains the full scope. No catalogue, S3 or backup implementation was
started during the stop checkpoint.

Catalogue inventory (a candidate design, not an accepted conclusion): a single
dedicated worker may fit the one connection and TEMP reservations better than
spawn_blocking. Preserve transaction bodies as owned jobs; bound queue count,
input/result bytes and lifecycle. Acquire permits before cloning inputs. A
detached SQL call alone is insufficient: reserve-room-edit/apply/refund,
reserve-object/upload/commit, checkpoint budget transfer, and publication
prepare/mutate/commit need service-owned compound lifetimes. Otherwise a dropped
caller can leak a reservation between SQL completion and guard installation.
Migrate actual production call sites, including raw with_connection users in
document/store, journal/store, maintenance and backup. Test blocked SQL,
saturation, cancellation at each boundary, shutdown and worker failure.

Remaining helper inventory: format-to-main filename, CommentView construction,
rendering publication lifecycle, UTF-16 overflow/surrogate behavior, request
digests preserving receipt compatibility, bulk comment snapshot callers, and
Peer.address/deployment-lock helper usage. Avoid abstractions without a shared
policy or ownership benefit. Backup requires actual exclusivity evidence.

## Environment

Root: `/home/vincent/repos/librepaper`. No applicable AGENTS.md was found.
Rust-engineer and code-reviewer skills were used. Main web assets were rebuilt
with `npm run build` in web to repair incomplete ignored dist assets. Some
worktrees show untracked `web/dist` symlinks to root assets; do not stage them.
Socket tests need execution outside the sandbox. Agents used independent target
directories under `/tmp`; retention used root target. The resident baseline used
`/tmp/librepaper-runtime-clippy`. Avoid competing Cargo builds on one target.
Retain worktrees until their branches are reviewed. No commits were pushed.
