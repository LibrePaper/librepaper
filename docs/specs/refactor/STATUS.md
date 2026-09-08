# Refactor implementation status

This checklist tracks the full scope of [the roadmap](../../../SPEC-refactor.md).
Partial deliveries do not complete their parent track. Update evidence after
integration; a branch's green tests alone do not establish a safe combined result.

| Track | State | Evidence or remaining work |
| --- | --- | --- |
| 1. Catalogue execution | Pending | Execution boundary, lifecycle, bounded inputs/results, and async caller migration all remain. |
| 2. Lock scopes | Partial, registry merged | `9ec5959`: registry scans release the map, cached lookups bypass admission, eviction revalidates identity/ownership. Legacy comments and catalogue waits remain. |
| 3. Shared retention | In progress | Isolated `refactor/retention`; needs concurrency/failure tests and review. |
| 4. Rendering lookup | Implemented; catalogue migration pending | `4e55fe9`, `5f8e9ea`: one joined candidate query; Fs metadata/S3 HEAD; 131 -> 1 connection operations on 130 events, zero PDF bodies for metadata, one for bytes. |
| 5. Resident estimates | Pending profiling | Profile after registry changes; decide from measured serialization cost. |
| 6. Write errors | Pending | Explicit mutator results, typed quota/storage/refusal errors, caller and transport mapping. |
| 7. Validated commands | Merged | `a86fbb7`, `24b0206`, `edf8d5f`: typed dispatch, early discriminator/target validation, preserved retries and correlation. |
| 8. Shared policies | Partial in progress | Content-identity accessor in retention branch. Other policy/helper inventories and extractions remain. |
| 9. Stable attribution | Review fixes requested | Base `81d53e8`; verify final resident completion after erasure and attribution retained in staged receipts. Not merged. |
| 10. Size limits | Review fixes requested | Base `f130518`; fix shared-token admission bypass and detached write ownership through room cancellation/eviction. Not merged. |
| 11. S3 operations | Pending | Retry bounds, ambiguous outcomes, per-object batch deletion and accounting. |
| 12. Backup ownership | Pending | Establish actual exclusivity mechanism before prescribing a guard API. |

## Integration evidence

At `edf8d5f`, the combined registry/rendering/command changes passed:

- `cargo test --workspace --offline`: 782 Komodoc library tests, 1 existing
  ignored test; all integration and other workspace tests passed.
- `cargo clippy --workspace --all-targets --all-features --offline -- -D warnings`.
- Formatting and diff checks.

Test socket listeners require execution outside the filesystem/network sandbox.
The first full run also found incomplete ignored `web/dist` assets; rebuilding
the existing web sources with `npm run build` resolved those five failures.
No test assertions were weakened to accommodate them.

## Review dispositions

The registry change preserves active `Arc<Room>` ownership by counting the scan's
one temporary reference explicitly and checking pointer identity immediately
before removal. Its new tests block a state lock deterministically and verify
cached retrieval and new request pins. No unresolved blocking finding remains
for that delivered portion.

The rendering change preserves the previous optional endpoint fallback when
the newest registered object is missing or unreadable. The blob API retains
error/absence distinctions. Direct PDF serving already downloaded once, so the
implementation did not add a redundant serving abstraction.

Command review initially requested changes because production converted typed
commands back into the wire representation. The followup removed those round
trips, retained receipt lookup before reply-body validation, and added malformed
target correlation coverage. Parent review additionally retained target IDs on
unknown-command errors. Verdict: approve the revised integrated contribution.

Size and attribution review findings remain blocking for their respective
branches until their fixes and deterministic regression evidence are reviewed.
Full-goal completion, final lock/call-site audit, and integrated failure testing
remain outstanding.
