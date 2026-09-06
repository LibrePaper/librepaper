# SPEC: sharing, and who may do what to a document

Status: the sharing model is built, and it is link-based. A document is
shared with links, not people: it holds at most three standing links, one per
role -- reader, commenter, editor -- and minting a role's link again rotates
it, killing the old key without losing the role it stood for. Each link
carries an expiry, six months by default, `never` if the owner asks for one
that does not expire. Visibility keeps its three values, `private`, `link`
(the default), and `listed`; a reader link only matters on a `private`
document, since a `link` or `listed` document is already readable by anyone
who has the URL. Editing requires an account wherever `--publishers` does: an
edit link authorizes, and the account behind the request attributes, so the
holder of an edit link edits once signed in as somebody `--publishers` allows,
and only comments until then. Under `--publishers anyone` the link edits as
it is, since that deployment asks nobody for a name. Anonymous commenters are told apart by a
stable per-document pseudonym (`pseudonym.rs`), deterministic from their
visitor key and the document's slug, so the same person reads as the same
name across a conversation without ever typing one in. A signed-in reader who
opens a document through a link is recorded as a guest of it, which is how the
document lands on that reader's own list without naming them the way a grant
does; a guest drops off once the link that brought them there is rotated or
revoked. Named grants -- an editor or a commenter added by GitHub login, from
before links existed -- are legacy: still honoured by `role_of`, still
revocable by that login, but a document never grows a new one; `--editor` and
`--commenter` no longer exist as ways to create one. `IndexEntry::role_of` and
`readable_by` in `store.rs` are the whole of the decision, and every route
asks them. What follows is what is left.

## Automation

There is no separate token for a script or a bot: a comment link or an edit
link is the bearer credential, the same one a person pastes into a browser.
Minting one for a person and one for a script is the same command, and
revoking a compromised script's access is the same rotation that kills a
leaked human link. What is still open is the pair of things that would make a
link fit a machine caller specifically, rather than whichever person a human
happened to forward it to: a rate budget of its own, separate from a human
reader's, and a label, so an owner reading `komodoc share`'s listing can tell
"the CI link" from "the one I sent the reviewer" instead of two identical
rows.

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
  department; left as address until it hurts. This is the same gap the
  automation section above wants closed for a machine caller specifically.
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

It is not a second identity provider. A legacy grant is a grant to a handle --
a GitHub login, or a Google account's verified email -- and the handle
resolves to an account through the providers the auth module has. Since a
grant can no longer be created, there is nothing left to build here: the old
gap, that only a GitHub login resolved before sign-in, is frozen along with
the grants it would have affected. New coauthors and reviewers are named with
a link instead.

It is not a permission system for the server, and it is not delegation: an
editor cannot share.
