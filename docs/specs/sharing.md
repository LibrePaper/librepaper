# SPEC: sharing, and who may do what to a document

Status: the sharing model is built. Roles, the ceiling against
`--publishers`/`--commenters`, grants by name and by link, the Share dialog,
visitor adoption and `private`/`link`/`listed` visibility all ship on the Rust
host; `IndexEntry::role_of` and `readable_by` in `store.rs` are the whole of the
decision and every route asks them. What follows is what is left.

## Scoped credentials for automation

An owner may mint a revocable bearer token scoped to one document and a role.
It is presented on the WebSocket upgrade or an HTTP request, without a browser
cookie. Token authentication feeds the same role function and the same server
ceilings as every other grant; it must not be mistaken for the existing GitHub
bearer token.

An editor token cannot bypass the publisher policy: where a named account is
required, the token must be bound to an allowed account rather than treated as
anonymous editing.

Store only the token's hash and its scope, role, label and rate budget. The
token supplies a distinct author key, so automated comments are attributed to
the automation client rather than to its issuer, and its rate budget is separate
from a human's. Revocation must stop further reads and writes, including on an
already connected socket. The client cannot claim its own identity or role in a
message.

Tests must cover document isolation, forbidden roles, attribution, rate limits,
and revocation on both HTTP and an open socket. The peer client and snapshot
interface are specified in `docs/specs/sync.md`.

## A front page for `listed` documents

`--no-listing` exists and `listed` respects it, but there is no public page to
browse other people's `listed` documents on: `/api/list` still takes a
publisher, so `listed` today means "in the list of anyone who can already
list". The landing page's `visible` filter is meant to be the examples,
everything the caller holds a role on, and everything `listed` — the last part
needs a route that answers without a publisher, and the front page to show it.

## Open questions still open

- **A `--link-lifetime` ceiling.** Links expire at six months by default and
  `--until never` is allowed for an owner who wants none. No operator ceiling
  was added; there is nothing yet asking for one.
- **Rate limits by link.** Comments are rate-limited by address. A link is a
  better key for one reviewer and a worse one for a link forwarded to a
  department; left as address until it hurts.
- **A private HTML document's own scripts do not run.** A private document is
  never served from the documents origin — that origin shares no cookie and so
  has no identity to check `private` against — so the reader paints the text in
  over the socket with `innerHTML`. A document that needs its scripts is one to
  share by link. Revisit only if that trade starts costing something real.

## What it is not

It is not a hiding place for comments. Every reader sees every comment. A blind
review in which reviewers cannot see one another's comments until the owner
reveals them is a visibility on comments rather than on documents, and is its
own spec — which `via` on each comment already makes possible.

It is not a second identity provider. A grant by name is a grant to a
handle -- a GitHub login, or a Google account's verified email -- and the
handle resolves to an account through the providers the auth module has.
What is not built: a grant to a handle the server has never seen. Only
GitHub logins resolve before sign-in, so `--editor anne@example.org` is
refused today with a message saying so; the fix is to store the unresolved
handle on the grant and resolve it in `Server::sign_in()`, the one place
both providers' callbacks pass through. When that is done, rename
`Grant.login` to `handle` with `#[serde(alias = "login")]`, since the field
has held a handle since providers arrived and the share dialog and
`revoke_from` read it as one.

It is not a permission system for the server, and it is not delegation: an
editor cannot share.
