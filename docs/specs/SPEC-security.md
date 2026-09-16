# LibrePaper security and privacy

*2026-09-15, finding 1 settled 2026-09-16. Threat model and proposed
specification. A finding marked settled describes what the code now does;
every other finding describes current behavior, and its requirements do not.*

## Purpose

State what LibrePaper protects, from whom, and where the protection rests on a
deployment decision rather than on code. The audit behind this document found
no unauthenticated bypass, no injection sink, and no missing origin check. The
dangers are structural: boundaries that are derived rather than asserted,
trust relationships the product does not name, and defaults that hand a
self-hoster a dependency they did not choose.

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
4. **A document** — the bytes a person uploads or writes — is *hostile*. It runs
   its own scripts, it is framed by the reader, and it may have been written by
   anyone with editor access.

The fourth principal is the one that matters. Every boundary below exists
because a document is code.

## Findings, by consequence

### 1. The document origin is asserted, not derived — settled

*Implemented. What follows describes the finding as it stood and the shape of
the answer, because the reasoning is what the other findings lean on.*

`server/origins.rs` used to build the isolation hostname by prefixing `docs.`
to whatever the `Host` header carried, and nothing checked that name against
anything. Published documents are still served byte for byte
(`document/html.rs`) under a CSP that permits `'unsafe-inline'`,
`'unsafe-eval'` and `https:` (`server/routes.rs:1110`), which is finding 2.

What that hostname is and is not holding back is worth stating precisely. Over
HTTPS the session cookie carries the `__Host-` prefix, so it is host-only and a
document on the sibling hostname never receives it, whatever the hostname
arrangement. The real exposures are same-site request forgery, which is what
rule A answers, and full same-origin DOM access if the two origins ever
collapse into one, against which the cookie prefix is worth nothing. Over plain
HTTP the prefix is dropped and a same-site document can plant a cookie under
the bare name, which `auth/mod.rs:180` names in a comment and refuses to read.

Nothing refused to start when the hostname was wrong. A deployment on a bare
host or an IP address, a proxy that rewrites `Host`, or an operator who
resolves a broken preview by pointing `docs.<host>` and `<host>` at one origin
could make the deployment fail closed or serve the document shell from the
wrong origin. The browser boundary is the scheme/host/port origin tuple; DNS
alone does not prove it. Rule A also cannot repair a request that reaches the
reader origin with a document response.

The `Host` header escaped into one more security-relevant string: the OAuth
redirect URI was built from it. A provider rejects an unregistered redirect
URI, so that failed closed, but it was a second place where an unvalidated host
reached somewhere it mattered.

Rule A (`cross_site_refused`) is load-bearing for every state-changing route
precisely because `docs.<host>` is same-site with the reader.

**Settled.** A deployment states its reader origin with `--origin`; the
document origin defaults to that host behind `docs.` and `--docs-origin`
overrides it. `Origins::configure` refuses at startup to accept two origins
sharing a host, naming the DNS record and certificate to create. It is a
deployment diagnostic and not a claim that DNS proves browser isolation. Every
request resolves its `Host` against that pair in the outermost middleware,
before anything is reserved or read, and an unrecognized host is answered 421
rather than served on a guess.

Three consequences beyond the refusal. Which side a request is on is now
decided when the host is matched rather than by reading `docs.` off the name
again, so a document origin on an unrelated host routes correctly. The scheme
is the configured origin's rather than a forwarded header's, which is what
decides `Secure` and the `__Host-` prefix, so `X-Forwarded-Proto` no longer
has a say. And the OAuth callback is built from the validated origin, so a
request cannot name its own redirect.

A deployment given no `--origin` answers on loopback alone, which is what
development and the test suite use. Loopback is accepted whatever the
configuration says, because the operator's own `admin status`, container
health checks and the tests all arrive that way, and a browser cannot be
induced to send a loopback `Host` to a remote server.

Published-document routes were already reachable only inside the document-side
branch of the router, so the guarantee that hostile document HTML never reaches
the reader origin reduces to the property the tests now assert directly:
nothing but the configured document origin ever resolves to the document side.

### 2. A shared link is a web page, not a paper

With the boundary intact, a document still reaches any `https:` host. It can
beacon who opened it, when, from which address, with a full fingerprint; it can
load remote resources that do the same passively; it can paint a convincing
sign-in panel inside its own frame. `form-action 'none'` and `frame-ancestors`
close the classic versions; `fetch()` is open by design.

For anonymous review this is a leak of the reviewer to the author, through a
document the author wrote.

**Required.** Every document, including one its reader owns, renders under a
strict policy by default. The policy is explicit and complete: it includes
`default-src 'none'` and `connect-src 'none'`; scripts and styles are allowed
only when required by the selected renderer, images and fonts are limited to
the deployment and `data:`, and `form-action 'none'`, `base-uri 'none'`,
`object-src 'none'`, explicit navigation rules, and the appropriate
`frame-ancestors` rule remain present. No external script, media, worker,
manifest, or navigation source is implicit. A permissive policy is an explicit,
per-document choice the reader makes and can withdraw; the reader is told which
policy is in force, and browser tests verify that the strict policy blocks
outbound connections and third-party resources.

The cost is real and belongs in the manual rather than in a reader's surprise.
Under this policy a document loses remote images, web fonts, and any
interactive widget that fetches its own data; the per-document permissive
choice is the answer, and it has to be discoverable. The in-frame agent is
unaffected: `web/src/agent/agent.js` makes no network calls of any kind and
talks to the reader only over postMessage, so it survives `connect-src 'none'`
intact.

### 3. The companion executes collaborator-authored code

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
- **Settled.** `Preset.environment` refuses loader and interpreter variables
  at validation time, before any route that could set them exists.
  `refused_environment` denies the `LD_*` and `DYLD_*` namespaces whole,
  because a loader honors a long and version-dependent list and enumerating it
  is a losing game, and denies by exact name for the interpreters the engines
  embed: Python and R through Quarto, Perl through biber and latexmk. It
  covers both the "load code from here" and the "run this code at startup"
  forms, since refusing only the first would be theatre. The check is
  case-insensitive and reads the environment map only, so an option that
  happens to share a name is unaffected. `TEXINPUTS` and its siblings are
  deliberately allowed: a preset configuring a compiler is the point of the
  feature, a TeX run is confined by its own shell-escape policy, and refusing
  them would break real configurations for nothing.
- Companion settings list live execution grants with their origin, document,
  trusted editor identities, approved source identity, entrypoint and expiry,
  and revoke them individually. Pairing tokens and execution grants are shown
  and revoked separately; a pairing does not imply permission to execute.

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
16 KiB of context, channels that expire after an hour of idleness rather than
an hour of life, no stored transcript) and the model runs on the user's own
machine under the user's own keys, which is the right privacy answer. Nothing
bounds what instructions hidden in a document can direct the agent to do with
its authority. That authority is already link-scoped
(`server/assistant.rs:42`), which is the correct instinct and the actual
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
cookies, `__Host-` cookie naming wherever the deployment is HTTPS, POST-only
logout behind rule A, path rules that refuse `..`, dotfiles, control characters
and deep trees (`document/paths.rs`), capabilities stored only as digests, 0600
and 0700 on every secret and state directory, atomic key creation,
`x-content-type-options` and `no-referrer` throughout, postMessage
authenticated at both ends — origin-checked inbound on the reader
(`web/src/components/Preview.svelte:50`), source-checked inbound in the frame
(`web/src/agent/agent.js:748`), and addressed to a named target origin rather
than `*` on each side — inert `template` parsing with resource attributes
stripped before the parser runs (`web/src/lib/diff-display.js:229`), no process
spawn on any request path (the operator's `admin backup` shells out to
`pg_dump`, which is not one), and no constructed SQL outside benchmark
teardown.

## Order of work

Finding 1 is done, and so is the loader-variable denial in finding 3. What is
left, in order:

1. The strict document policy, finding 2. The largest piece of work here and
   the only finding a hostile document author can act on with no operator
   mistake at all.
2. Backup encryption, finding 4.
3. The rest of finding 3: grants that name the editors and the source identity
   they trust, and a settings surface that lists and revokes them apart from
   pairings. Larger than the denial above, and it needs a design decision about
   what counts as an approved source identity.
4. Release signing and the second secrets recipient, finding 7.
5. Everything else, in the order the manual needs it.
