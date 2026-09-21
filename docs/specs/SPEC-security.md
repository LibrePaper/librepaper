# LibrePaper security and privacy

*Reassessed 2026-09-20 against the current working tree after the server-as-log
cutover; speculative consent tracking and link-lifetime changes pruned on
2026-09-21. This is a security contract and decision backlog, not a fresh security
audit. Current behavior, proposed changes and unresolved choices are separated
below; proposals are not implemented guarantees.*

## Purpose and architectural scope

Protect document and account isolation, local execution consent, recoverable
work and deployment availability while keeping the application small enough to
operate on a modest VPS.

The baseline is one Rust server, PostgreSQL and blob storage, browser-side
compilation, batched source persistence and evictable CRDT state. Readers follow
live source projections; there is no longer a published rendered-bundle system.
Local builds and agent interfaces remain supported. Removing or simplifying
those workflows is a product decision, not an assumption this spec can make.

Read this alongside the [simplification audit](../simplification-audit.md),
[architecture recommendations](../../REVIEW-BIG-IDEAS.md),
[frugal scaling proposals](../../SPEC-frugal.md) and
[resource inventory](../resource-bounds.md). The latter two inform the
availability work below; they are not evidence that every proposed bound or
optimization already exists. Source references below are relative to
`crates/librepaper/src/` unless they name another root.

## Trust model and contracts to preserve

- **The operator is trusted with plaintext.** They control the binary,
  database and storage and can read documents and identities. There is no
  end-to-end encryption, and none is proposed here. Backup encryption protects
  a copied recovery point, not a document from its operator.
- **Document access is scoped authority.** Owners control sharing; a link is a
  bearer credential with a role, expiry and revocation. Forwarding it forwards
  its authority. Local code execution requires a separate consent boundary.
- **Documents are hostile input and may contain executable code.** HTML runs
  in the document origin; companion builds may execute code on the machine
  running them. Neither possession of a document link nor permission to edit
  implies permission to execute its contents locally.
- **Agents consume hostile content too.** Document text and comments can carry
  instructions. Link-scoped authority limits the damage within LibrePaper;
  it does not constrain unrelated tools or credentials in an external agent.
- **Availability is part of security.** An authorized editor or commenter can
  still exhaust shared resources. Per-request and per-document limits must
  compose into meaningful deployment limits.

Retain separate application and document hosts, authorization at the durable
semantic-command boundary, revocation enforcement, honest durability signals,
and recovery of unsynced work. Simplification must preserve these contracts or
explicitly identify the user promise being changed.

## 1. Rendering isolation: retained boundary, changed script policy

**Current.** `server/origins.rs` validates the deployment origins and refuses
application and document origins on the same host. The preview frame remains
sandboxed; `web/src/components/Preview.svelte` checks both the sending origin
and frame window and addresses outgoing messages to a named origin.

The production `serve_shell` and `serve_viewer` responses in `server/routes.rs`
allow `https:` in `script-src`. The stricter `document_policy` that excludes
remote scripts is now compiled only under `#[cfg(test)]`. Its unit tests test
that unused policy, not the headers readers actually receive.

The previous spec's claim that documents cannot load third-party code is
therefore false for the production policy. Its claims about code being fixed
at publication and publishing reporting external hosts also do not describe
the current live-source architecture. The separate-origin boundary remains;
it must not be confused with a restriction on outbound requests.

**Decision and recommendation.** Decide whether remote scripts are supported
in rendered documents. Prefer embedded or digest-pinned dependencies where
compatible with the retained formats, but investigate the actual rendering
paths before tightening CSP. If remote scripts remain allowed, explicitly
accept that their hosts can observe requests and change executable content
independently of a source revision. Images, fonts and network requests also
remain privacy exposures even if remote scripts are disallowed.

**Next work.** Test the actual shell and viewer response headers, consolidate
the intended production policy, and remove the misleading test-only policy.
Update the privacy documentation to match the decision. A static artifact or
source digest alone would not freeze remote dependencies.

## 2. Companion execution: preserve consent before simplifying grants

**Current.** Companion presets, folder bindings and isolated workspaces still
serve local builds, including projects with unshared inputs. Quarto can execute
R and Python. Preset environment validation refuses loader and interpreter
variables (`local/presets/mod.rs`), but that does not make document code safe.

`PresetGrant` binds origin, project, preset, workspace mode, operation,
entrypoint and preset semantic revision. Changing the preset invalidates its
grant. The store already supports listing and revoking grants. These grants do
not bind authenticated collaborator identities or an approved source revision,
and do not carry an expiry. Quarto folder bindings separately carry an
`execution_granted` flag. Pairing credentials and execution permissions are
therefore related but distinct mechanisms.

**Recommendation.** First name the supported execution modes and the meaning
of consent. Options include explicit execution of a captured revision, or a
clearly disclosed continuing trust grant for a project. A continuing grant
must explain that collaborators can change the code that will run. Determine
expiry and invalidation rules for the chosen mode; do not silently broaden a
grant when its scope changes. Validate that revision approval covers the actual
inputs executed, including relevant local inputs and build configuration.

Expose active execution permissions and revocation separately from pairings,
reusing the existing grant store where possible. Do not remove frozen-cache
checks or execution gates merely because another local-build abstraction is
being removed. Choose the contract before building collaborator identity
tracking or a second grant system.

## 3. Backups: plaintext recovery points remain an exposure

**Current.** `storage/backup.rs` takes a snapshot-consistent PostgreSQL dump,
copies referenced immutable objects and writes a manifest with integrity
checks. Files and directories use private permissions. The recovery point is
not encrypted. Checksums detect corruption; they do not provide confidentiality
or authenticate a backup against an attacker who can replace its manifest.

**Recommendation.** Establish a documented encrypted backup and restore path.
Choose between built-in recipient encryption and an operator-managed encrypted
backup tool before adding key management to LibrePaper. If built-in encryption
is chosen, make plaintext export an explicit opt-out. If encryption is external,
state exactly where plaintext staging exists and who must protect and remove
it. Do not describe the existing command as encrypted in either case.

Verify recovery with the encryption layer and its keys, not just successful
archive creation. Preserve snapshot consistency and object completeness when
exploring incremental backups. Document separately that the operator can read
live data and that restored backups can reintroduce previously deleted data.

## 4. Sharing: retain the existing lifetime and renewal behavior

**Current.** `LINK_DEFAULT_SECONDS` in `server/sharing.rs` remains 180 days.
The sharing dialog offers 7 days, 30 days, 6 months and Never. Its settings path
can update an existing link's expiry without replacing the key; the dialog uses
that path. Rotation is a separate action. The old statement that renewal must
mint a replacement and drop guests is obsolete.

Keep the current default. No observed access problem or review-workflow evidence
justifies shortening it; doing so would also increase unexpected expiry. The
30-day-default proposal is removed from the backlog. Renewal needs no new
backend operation.

Preserve role scope, expiry checks and prompt revocation regardless of the
default. Forwarding a valid link is an accepted property of bearer sharing,
not something a shorter lifetime prevents.

## 5. Privacy: distinguish editor presence from reading

**Current.** The application has a persistent signed visitor credential.
However, reader/commenter sessions now use the annotation socket without
joining source collaboration (`web/src/lib/reader/collaboration.js` and
`web/src/components/Reader.svelte`). The server accepts `doc-presence` only
from editors and relays it to other editors (`server/socket.rs`). The previous
blanket claim that every reader broadcasts their cursor to the author is no
longer accurate.

This does not establish anonymous reading: authored document resources can
still contact outside hosts, the operator sees requests, and comments carry
identity information. Editor presence remains a separate disclosure question.
If an invisible mode is proposed, define whether it hides editor identity,
caret position, connection counts or all three; it cannot promise network
anonymity or conceal the authorship of a write.

**The LaTeX mirror.** Browsers fetch the compiler and packages from the
configured mirror, by default the project mirror. Digest verification protects
integrity, not request privacy or availability. The public
[hosting manual](../../site/host.md#privacy-and-the-latex-mirror) now explains
the dependency and `--latex-mirror`; documentation is no longer absent. A
concrete self-hosting recipe or direct link to the mirror layout/build guide
is still needed. Do not describe document source as being uploaded there.

**Next work.** Reconcile [docs/privacy.md](../privacy.md) and the public
[privacy page](../../site/privacy.md) with actual rendering and presence
behavior. Disclose outbound requests and editor visibility before proposing
additional privacy controls. Browser-first rendering makes mirror requests
relevant to readers as well as editors.

## 6. Supply chain: distinguish integrity, provenance and recovery

**Current.** `deploy/install.sh` and `deploy/install-companion.sh` check release
archives against `checksums.txt` from the same release. The release workflow
uses ad-hoc macOS signing. The repository's SOPS secrets file has one PGP
recipient. Wasm modules are pinned by digest in `wasm-modules.lock`.

**Recommendation.** Authenticate releases with a verification identity that
installers trust independently of the downloaded checksum file. Specify key
rotation and recovery, and test installer rejection of invalid releases.
Signing with credentials available to a compromised build job does not by
itself solve pipeline compromise; state which attacks the chosen scheme
addresses. Digest pinning alone is not release provenance either.

Document secret rotation and recovery as operator procedures, recording dates
without recording secret values. A second recipient is one recovery option,
not a universal requirement: it also adds another decryption authority. Choose
it according to the operator's custody and recovery arrangements.

## 7. Agent writes: attribution is partly implemented, not prevention

**Current.** `server/mod.rs::resolved_role` keeps automation authority bounded
by the presented link rather than the caller's broader account permissions.
Agent source patches run through semantic commands, and
`room/agent.rs::AgentPatchCommand::persist` records a label with reason `agent-patch`
and author information. Explicit agent checkpoints also exist. The claim that
agent writes have no history marking at all is outdated.

**Remaining question.** Verify consistent attribution for source patches,
comments, replies, suggestions and decisions, including what the history and
comment interfaces actually display. A label on one patch path does not prove
that every agent-originated write is recognizable. Prefer extending existing
command/author metadata over introducing another audit subsystem. Distinguish
server-known agent entry points from a guarantee of detecting all automation.

LibrePaper does not claim to prevent indirect prompt injection. Preserve
bounded authority and transactional authorization; attribution helps detection
and recovery, not prevention. External agents and locally launched runners may
use remote model providers and broader tools, so local execution of a runner
must not be described as a guarantee that document context stays on the machine.
Retain these protections whichever assistant entry points survive simplification.

## 8. Availability and authorization under frugal scaling

These concerns were missing from the earlier spec and belong in its security
scope. The detailed implementation inventory lives in
[resource-bounds.md](../resource-bounds.md); keep that as the source for exact
limits rather than duplicating a second table here.

**Deployment bounds.** Per-document pending-update ceilings do not establish
a global pending-write bound. Audit pending and in-flight byte ownership,
transient CRDT builds, comment caches and response serialization across many
documents. Add missing aggregate accounting where evidence warrants it. Under
pressure, refuse or defer work explicitly and retryably while clients retain
unsynced work. Never report a refused or merely buffered write as durable.

**Admission versus reading.** Configured comment/reply creation caps and the
comment-specific rate setting are not currently enforced as their names
suggest. Pagination and read-memory guards address a different problem from
write admission. Track the ongoing transport work separately; it must not be
mistaken for an enforced creation quota. Any admission policy must cover browser
and agent writes at the authorized transaction boundary, handle concurrent
last-slot requests and retries, and keep older over-limit documents readable.

**Authorization caching.** Established sockets currently refresh authorization
through `reauthorize_connection`, with a one-second cache. Reducing that query
traffic is attractive, but an in-memory replacement needs complete invalidation
for sharing, ownership, session and account changes, local expiry checks, and
a defined response to database changes made outside the owning process. Keep
transactional checks for durable semantic commands. A missed invalidation must
not turn a performance optimization into continuing access after revocation.

**Verification.** Exercise many idle readers, many actively edited documents,
a crowded document, reconnect bursts and database slowdowns. Measure total
memory, pending bytes, refusals and durable-save latency as well as throughput.
Test revocation on an already-open socket and immediately before a command
commits. Prefer the existing workload harnesses and one authoritative policy
path over a new general-purpose security framework.

## Non-goals and accepted limits

- End-to-end encryption or protection from the deployment operator.
- Making arbitrary document code safe by inspecting or sanitizing it.
- Preventing a valid link holder from forwarding their credential.
- Claiming anonymous review while authored resources can contact outside hosts.
- Preventing prompt injection in external agents or controlling their other tools.
- Adding distributed security coordination before the single-writer deployment
  model changes. Future document sharding would need ownership fencing and
  cross-owner revocation semantics of its own.

## Recommended order of work

1. Correct the rendering-policy discrepancy and test production headers; update
   the privacy claims that depend on it. Decide whether to restore the stricter
   policy or explicitly support remote scripts.
2. Define local execution consent for the workflows being retained. Preserve
   existing gates while that decision is open.
3. Address measured aggregate resource gaps and preserve revocation correctness
   in any authorization-cache work. Coordinate with the current transport work.
4. Establish encrypted backup recovery and authenticated release verification
   as independent, bounded operational improvements.
5. Finish consistent agent attribution and the mirror/presence documentation.

This ordering is a recommendation, not a new implementation mandate. The older
review's broad “already sound” statements are not a current audit certificate;
retain focused tests on real enforcement paths rather than carrying those
statements forward without re-verification.
