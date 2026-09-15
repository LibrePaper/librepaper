# LibrePaper security and privacy

*2026-09-15. Threat model and proposed specification; the findings describe
current behavior, the requirements do not.*

## Purpose

State what LibrePaper protects, from whom, and where the protection rests on a
deployment decision rather than on code. The audit behind this document found
no unauthenticated bypass, no injection sink, and no missing origin check. The
dangers are structural: boundaries that are derived rather than asserted,
trust relationships the product does not name, and defaults that hand a
self-hoster a dependency they did not choose.

This complements [REVIEW-architecture.md](REVIEW-architecture.md). Where that
document explains how the parts fit, this one explains what happens when one of
them is wrong.

## Trust model

Four principals, in decreasing order of what they are allowed to assume:

1. **The operator** runs the binary and the database. They see every draft,
   comment, identity and presence event in the clear. There is no end-to-end
   encryption and none is proposed here.
2. **The signed-in owner** of a document holds every authority over it.
3. **A link holder** holds exactly the role their link names, for as long as the
   link lives. The link is the credential; possession is the grant.
4. **A document** — the bytes a person uploads or writes — is *hostile*. It runs
   its own scripts, it is framed by the reader, and it may have been written by
   anyone with editor access.

The fourth principal is the one that matters. Every boundary below exists
because a document is code.

## Findings, by consequence

### 1. The document origin is derived, not asserted

`server/origins.rs` builds the isolation hostname by prefixing `docs.` to
whatever the `Host` header carries. Published documents are then served byte
for byte (`document/html.rs`) under a CSP that permits `'unsafe-inline'`,
`'unsafe-eval'` and `https:` (`server/routes.rs:1105`). The only thing between
a document's scripts and the session cookie is that hostname.

Nothing refuses to start when the hostname is wrong. A deployment on a bare
host or an IP address, a proxy that rewrites `Host`, or an operator who
resolves a broken preview by pointing `docs.<host>` and `<host>` at one origin,
collapses the boundary silently. The result is account takeover for anyone who
opens a shared link. A different port would not help and the module says so;
what it does not do is check.

Rule A (`cross_site_refused`) is load-bearing for every state-changing route
precisely because `docs.<host>` is same-site with the reader.

**Required.** The server asserts its document origin at startup: the configured
or derived document host must resolve to this deployment, must differ from the
reader host, and must not be reachable as the reader host. Failure is a refusal
to start with a message naming the DNS record and certificate to create, not a
warning. A single-origin deployment is not a supported configuration.

### 2. A shared link is a web page, not a paper

With the boundary intact, a document still reaches any `https:` host. It can
beacon who opened it, when, from which address, with a full fingerprint; it can
load remote resources that do the same passively; it can paint a convincing
sign-in panel inside its own frame. `form-action 'none'` and `frame-ancestors`
close the classic versions; `fetch()` is open by design.

For anonymous review this is a leak of the reviewer to the author, through a
document the author wrote.

**Required.** Documents a reader does not own render under a strict policy by
default — no `connect-src`, images and fonts from the deployment and `data:`
only — with the permissive policy an explicit, per-document choice the reader
makes and can withdraw. The reader states which policy is in force.

### 3. The companion executes collaborator-authored code

`librepaper local` runs Quarto, TeX and the other engines on the user's
machine. Quarto executes R and Python chunks. The loopback surface is sound:
loopback bind, `host_allowed` against DNS rebinding, a six-digit code behind a
constant delay and a rate limit, origin-bound tokens stored only as hashes,
per-`(origin, project, entrypoint)` preset grants, separate folder bindings,
and Origin plus `Sec-Fetch-Site` plus nonce on the management page.

The risk is the sanctioned path. Document-level sharing is not a trust boundary
for code execution: an editor on a Quarto project can put arbitrary code in a
chunk, and it runs on every other editor's machine at the next preview. The
grant is per document, thirty days, and silent after the first approval.

**Required.**

- A grant for an execution-capable format names the editors it trusts, not only
  the document. A new editor on a granted project suspends execution until the
  person who granted it approves again.
- `Preset.environment` refuses loader and interpreter variables (`LD_PRELOAD`,
  `LD_LIBRARY_PATH`, `DYLD_*`, `PYTHONPATH`, `PERL5LIB`, `R_LIBS*`) at
  validation time, before any route that could set them exists.
- Companion settings list live execution grants with their origin, document,
  entrypoint and expiry, and revoke individually. This exists for pairings and
  bindings; it must exist for execution.

### 4. Content is plain at rest, including backups

`storage/backup.rs` writes `pg_dump` output to a 0600 file and copies blob
objects beside it. File permissions are correct throughout the tree; encryption
is absent. A backup archive is the whole deployment in the clear, and it is the
artifact most likely to leave the host.

**Required.** `admin backup` encrypts the archive to an operator-supplied key
and refuses to write an unencrypted one without an explicit flag. The hosting
documentation states, in the operator's own words, that the operator can read
every document.

### 5. Link lifetime outlives its purpose

`LINK_DEFAULT_SECONDS` is 180 days and `never` is offered
(`server/sharing.rs:17`). The rationale in that module is that a round of
review has an end; the default is longer than most of them. Keys are stored
hashed, `referrer-policy: no-referrer` is set, and revocation and rotation both
work — the exposure is the forwarded mail, not the protocol.

**Required.** The default becomes 30 days with renewal offered from the sharing
dialog. `never` remains available and is labelled as what it is.

### 6. Privacy leaks the product does not name

- **The TeX mirror.** `web/src/lib/latex.js:38` sends every browser to
  `https://latex.librepaper.workers.dev/`. A self-hosted deployment still
  leaks each user's address and the exact set of TeX packages their document
  pulls — a usable fingerprint of the document — to a third party the operator
  never chose. Bytes are sha256-verified (`web/src/lib/latex/resources.js`), so
  this is a privacy and availability dependency, not an integrity hole.
- **Visitor identity and presence.** An anonymous reader receives a persistent
  signed `visitor:` credential (`server/signin.rs:9`), and the collaboration
  layer broadcasts presence and cursor position. A reviewer reading a paper is
  visible to its author in real time.
- **Attribution.** `attributed_as` keeps a pseudonym separate from the account
  id, correctly. The account id still travels with the write, so a pseudonymous
  comment is pseudonymous to other users and to nobody else.

**Required.** The mirror is a first-class deployment setting with a documented
self-hosting path, and the manual states the leak for anyone who keeps the
default. Presence is visible to a reader before they are visible through it,
and a reader may attend without broadcasting. The manual says what a pseudonym
does and does not hide.

### 7. Supply chain

`deploy/install.sh` and `deploy/install-companion.sh` verify the release
archive against a `checksums.txt` fetched from the same release. That detects
corruption, not a compromised pipeline or account. The macOS application is
ad-hoc signed. The wasm engines are the good pattern — pinned by digest in
`wasm-modules.lock`, refused on mismatch — and the release binaries are not
held to it. `deploy/keys.yaml` holds live OAuth client secrets and a Cloudflare
token encrypted to a single PGP recipient, with no second recipient and no
rotation record.

**Required.** Releases are signed and the installers verify the signature, not
only the digest. The secrets file carries a second recipient. Rotation is a
documented procedure with a date, because the first rotation will happen under
pressure.

### 8. Indirect prompt injection is unbounded

`server/mcp.rs` and the chat relay feed document text and comments — hostile
content by the definition above — to an agent that proposes edits and posts
comments. The relay is well bounded in size and lifetime (`server/chat.rs`:
16 KiB of context, one-hour channels, no stored transcript) and the model runs
on the user's own machine under the user's own keys, which is the right privacy
answer. Nothing bounds what instructions hidden in a document can direct the
agent to do with its authority. That authority is already link-scoped
(`server/assistant.rs:41`), which is the correct instinct and the actual
mitigation.

**Required.** Link-scoped agent authority is documented as a security property
and covered by a test that fails if an owner session ever widens it. Agent
writes are marked as agent writes in the timeline, so an injected edit is
visible as one. LibrePaper does not claim to prevent injection.

## Non-goals

- End-to-end encryption. The operator is trusted; the documentation says so.
- Preventing a document from being hostile. Documents are code, and isolation,
  not inspection, is the answer.
- Sanitizing uploaded HTML. The renderer is the identity, deliberately.
- Defeating a link holder who forwards their link.

## What is already sound

Recorded so the list above reads as a set of decisions rather than a verdict:
HMAC with domain separation and constant-time verification, PKCE, state
cookies, `__Host-` naming, POST-only logout behind rule A, path rules that
refuse `..`, dotfiles, control characters and deep trees
(`document/paths.rs`), capabilities stored only as digests, 0600 and 0700 on
every secret and state directory, atomic key creation, `x-content-type-options`
and `no-referrer` throughout, postMessage checked on both sides
(`web/src/agent/agent.js:740`, `web/src/components/Preview.svelte:50`), inert
`template` parsing with resource attributes stripped before the parser runs
(`web/src/lib/diff-display.js:229`), no server-side process spawn, and no
constructed SQL outside benchmark teardown.

## Order of work

1. The startup assertion in finding 1. Worst failure, cheapest fix.
2. The loader-variable denial in finding 3, before a route can reach presets.
3. Backup encryption, finding 4.
4. The strict document policy, finding 2.
5. Everything else, in the order the manual needs it.
