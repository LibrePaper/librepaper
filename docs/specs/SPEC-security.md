# LibrePaper security and privacy

*2026-09-15, last trimmed 2026-09-16. Threat model and proposed specification.
Every finding here describes current behavior; its requirements do not. Settled
findings are removed, and what is left is renumbered, so this is a list of work
remaining rather than a record of what was done.*

## Purpose

State what LibrePaper protects, from whom, and where the protection rests on a
deployment decision rather than on code. The audit behind this document found
no unauthenticated bypass, no injection sink, and no missing origin check. The
dangers are structural: boundaries that are derived rather than asserted,
trust relationships the product does not name, and defaults that hand a
self-hoster a dependency they did not choose.

Four things this document used to ask for are done, and the reasoning moved to
where it is enforced rather than staying here. The two origins a deployment
answers on live in `server/origins.rs`; the policy every published document
renders under is `document_policy` in `server/routes.rs`; the environment a
preset may not set is `refused_environment` in `local/presets`; and the bound
on an agent's authority is `resolved_role` in `server/mod.rs`. What a reader
of a hosted document should expect is in [privacy.md](../privacy.md), and what
an operator must configure is in [hosting.md](../hosting.md).

This complements the architecture review. Where that document explains how the
parts fit, this one explains what happens when one of them is wrong. (The
several specifications under `docs/specs/` that link to `REVIEW-architecture.md`
all point at a file that is not in the repository; the link is left out here
rather than repeated.)

## Trust model

Four principals, in decreasing order of what they are allowed to assume:

1. **The operator** runs the binary and the database. They see every draft,
   comment, identity and presence event in the clear. There is no end-to-end
   encryption and none is proposed here.
2. **The signed-in owner** of a document holds every authority over it.
3. **A link holder** holds exactly the role their link names, for as long as the
   link lives. The link is the credential; possession is the grant.
4. **A document** -- the bytes a person uploads or writes -- is *hostile*. It runs
   its own scripts, it is framed by the reader, and it may have been written by
   anyone with editor access.

The fourth principal is the one that matters. Every boundary below exists
because a document is code.

## Findings, by consequence

### 1. The companion executes collaborator-authored code

`librepaper local` runs Quarto, TeX and the other engines on the user's
machine. Quarto executes R and Python chunks. The loopback surface is sound:
loopback bind, `host_allowed` against DNS rebinding, a six-digit code behind a
constant delay and a rate limit, origin-bound tokens stored only as hashes,
per-`(origin, project, entrypoint)` preset grants, separate folder bindings,
and Origin plus `Sec-Fetch-Site` plus nonce on the management page.

The risk is the sanctioned path. Document-level sharing is not a trust boundary
for code execution: an editor on a Quarto project can put arbitrary code in a
chunk, and it runs on every other editor's machine at the next preview. Pairing
tokens currently expire after thirty days; that is separate from an execution
grant, which must have its own expiry and revocation semantics. Neither should
silently authorize a newly arriving collaborator.

**Required.**

- A grant for an execution-capable format names the authenticated editors it
  trusts, the project and entrypoint, and the source revision or content
  identity it approved. An editor link that is forwarded does not silently add
  a trusted editor. A newly authenticated or newly granted editor, or a source
  revision outside the approved identity, suspends execution until the person
  who granted it approves again.
- Companion settings list live execution grants with their origin, document,
  trusted editor identities, approved source identity, entrypoint and expiry,
  and revoke them individually. Pairing tokens and execution grants are shown
  and revoked separately; a pairing does not imply permission to execute.

### 2. Content is plain at rest, including backups

`storage/backup.rs` writes `pg_dump` output to a 0600 file and copies blob
objects beside it. File permissions are correct throughout the tree; encryption
is absent. A backup archive is the whole deployment in the clear, and it is the
artifact most likely to leave the host.

**Required.** `admin backup` encrypts the archive to an operator-supplied key
and refuses to write an unencrypted one without an explicit flag. The hosting
documentation states, in the operator's own words, that the operator can read
every document.

### 3. Link lifetime outlives its purpose

`LINK_DEFAULT_SECONDS` is 180 days and `never` is offered
(`server/sharing.rs:17`). The rationale in that module is that a round of
review has an end; the default is longer than most of them. Keys are stored
hashed, `referrer-policy: no-referrer` is set, and revocation and rotation both
work -- the exposure is the forwarded mail, not the protocol.

**Required.** The default becomes 30 days with renewal offered from the sharing
dialog. `never` remains available and is labelled as what it is.

### 4. Privacy leaks the product does not name

- **The TeX mirror.** `web/src/lib/latex.js:38` sends every browser to
  `https://latex.librepaper.workers.dev/`. A self-hosted deployment still
  leaks each user's address and the exact set of TeX packages their document
  pulls -- a usable fingerprint of the document -- to a third party the operator
  never chose. Bytes are sha256-verified (`web/src/lib/latex/resources.js`), so
  this is a privacy and availability dependency, not an integrity hole.
- **Visitor identity and presence.** An anonymous reader receives a persistent
  signed `visitor:` credential (`server/signin.rs:9`), and the collaboration
  layer broadcasts presence and cursor position. A reviewer reading a paper is
  visible to its author in real time.
**Required.** The mirror has a documented self-hosting path; it is already a
deployment setting (`--latex-mirror`), and the only pointer to hosting one is a
URL inside a startup error message. Presence is visible to a reader before they
are visible through it, and a reader may attend without broadcasting.

### 5. Supply chain

`deploy/install.sh` and `deploy/install-companion.sh` verify the release
archive against a `checksums.txt` fetched from the same release. That detects
corruption, not a compromised pipeline or account. The macOS application is
ad-hoc signed. The wasm engines are the good pattern -- pinned by digest in
`wasm-modules.lock`, refused on mismatch -- and the release binaries are not
held to it. `deploy/keys.yaml` holds live OAuth client secrets and a Cloudflare
token encrypted to a single PGP recipient, with no second recipient and no
rotation record.

**Required.** Releases are signed and the installers verify the signature, not
only the digest. The secrets file carries a second recipient. Rotation is a
documented procedure with a date, because the first rotation will happen under
pressure.

### 6. Indirect prompt injection is unbounded

`server/mcp.rs` and the chat relay feed document text and comments -- hostile
content by the definition above -- to an agent that proposes edits and posts
comments. The relay is well bounded in size and lifetime (`server/chat.rs`:
16 KiB of context, channels that expire after an hour of idleness rather than
an hour of life, no stored transcript) and the model runs on the user's own
machine under the user's own keys, which is the right privacy answer. Nothing
bounds what instructions hidden in a document can direct the agent to do with
its authority. That authority is already link-scoped
(`server/assistant.rs:42`), which is the correct instinct and the actual
mitigation.

**Required.** Agent writes are marked as agent writes in the timeline, so an
injected edit is visible as one. This touches the comment and history data
model rather than a single decision, which is why it outlived the rest of this
finding. LibrePaper does not claim to prevent injection.

## Non-goals

- End-to-end encryption. The operator is trusted; the documentation says so.
- Preventing a document from being hostile. Documents are code, and isolation,
  not inspection, is the answer.
- Sanitizing uploaded HTML. The renderer is the identity, deliberately.
- Defeating a link holder who forwards their link.

## What is already sound

Recorded so the list above reads as a set of decisions rather than a verdict.
What this document used to ask for and no longer does, first:

- **Two configured origins**, validated on every request in the outermost
  middleware, with a startup refusal when the reader and document origins share
  a host, and published documents reachable only on the second.
- **One policy for every published document**: it may run its own code and may
  not fetch code from another host. Quarto renders self-contained so it needs
  none, and publishing reports the hosts of a document that does. Images,
  fonts and connections deliberately still reach the open web, which is an
  accepted leak rather than an oversight.
- **Preset environments refuse loader and interpreter variables**, at
  validation time, before any route that could set one exists.
- **An agent's authority is the link it was given**, with a test that fails if
  a caller's own session ever widens it.

And, from the beginning: HMAC with domain separation and constant-time
verification, PKCE, state cookies, `__Host-` cookie naming wherever the
deployment is HTTPS, POST-only logout behind rule A, path rules that refuse
`..`, dotfiles, control characters and deep trees (`document/paths.rs`),
capabilities stored only as digests, 0600 and 0700 on every secret and state
directory, atomic key creation,
`x-content-type-options` and `no-referrer` throughout, postMessage
authenticated at both ends -- origin-checked inbound on the reader
(`web/src/components/Preview.svelte:50`), source-checked inbound in the frame
(`web/src/agent/agent.js:748`), and addressed to a named target origin rather
than `*` on each side -- inert `template` parsing with resource attributes
stripped before the parser runs (`web/src/lib/diff-display.js:229`), no process
spawn on any request path (the operator's `admin backup` shells out to
`pg_dump`, which is not one), and no constructed SQL outside benchmark
teardown.

## Order of work

1. Backup encryption, finding 2. Self-contained, and it blocks nothing else.
2. The rest of finding 1: grants that name the editors and the source identity
   they trust, and a settings surface that lists and revokes them apart from
   pairings. It needs a design decision first, about what counts as an approved
   source identity.
3. Link lifetime, finding 3. Extending an existing grant in place should come
   before shortening the default: there is no renew operation today, minting
   replaces the row and drops its guests, so a shorter default alone would
   force a redistribution every month and push people to `never`.
4. Release signing and the second secrets recipient, finding 5.
5. Marking agent writes in the timeline, finding 6.
6. The presence and mirror items in finding 4, in the order the manual needs
   them.
