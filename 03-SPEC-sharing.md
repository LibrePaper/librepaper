# SPEC: sharing, and who may do what to a document

Status: step 1 built on the Rust host; steps 2 to 6 not. See "What was built"
at the end. Extends the existing Rust server and its ownership checks.
Depends on `01-SPEC-history.md` for the way
documents are read, and changes one line of it; reads that spec's "Reading
without stored HTML" first.

## The problem

Rights are decided in two places today, and neither is the document.

The server has two switches. `--publishers` says who may create a document,
`--commenters` who may annotate one, each as a list of GitHub logins, `any`
signed-in account, or `anyone` at all. They apply to every document alike.

The document has one fact: its owner, recorded as `publisher` and
`publisher_id` on the index entry, keyed on a GitHub account's numeric id
or, on a deployment that asks for no sign-in, on the visitor cookie of the
browser that uploaded it. `owned_by` is the whole of the per-document
policy: the owner may replace, delete and edit the source, and `can_moderate`
lets the owner delete anyone's comment. Everyone else may read, because the
slug carries a random suffix and the link is the key, and may comment if the
server's switch says so.

So there is no way to name a coauthor. "Several people can edit at once" is
true of the same account in two tabs, and of the local trial, where a
document with no recorded publisher belongs to everyone. There is no way to
give a reviewer more than the link, or less than the whole internet: a draft
that must stay among four people is kept private by nobody guessing the
slug, which is a good lock with no door. And there is no way to give a
reviewer a name to comment under without a GitHub account, which is what
most reviewers of an academic paper do not have, or to keep a reviewer's
name from the author, which is what a blind review requires.

`01-SPEC-history.md` makes this sharper. Readers join the live session and see
the source; editors' checkpoints are recorded with a `by`, which that spec
admits "attributes to nobody in particular while a document has one owner".
The moment a document can have coauthors, `by` means something, and the
moment readers receive the source, "who may read" is a question with a cost
attached.

## Roles

Four, as a ladder: each includes the ones beneath it.

| role | may |
| --- | --- |
| **reader** | open the document, read its comments, view any checkpoint |
| **commenter** | comment, reply, resolve; delete their own |
| **editor** | edit the source; name, restore and pin checkpoints; delete any comment |
| **owner** | share, transfer, destroy |

The owner is one account and one only. Storage quota is counted against the
owner, including the checkpoints an editor causes; two owners would leave
the quota and the destroy command with no answer to "whose", for no gain
an editor does not already give. `komodoc transfer c9k alice` moves the
document, its history, its comments and its quota to another account, which
must satisfy `--publishers`.

Whether a caller holds a role is answered by one function that replaces
`owned_by` and `can_moderate`: given the identity the request carries and the
link key it may carry, return the highest role. Every route asks it; nothing
else decides.

## Two ways to hold a role

**By name.** A GitHub account, recorded by numeric id with the login kept
for display, exactly as ownership is now. A grant by name survives a rename,
follows the person across browsers, and puts a real name on every checkpoint
and comment.

**By link.** A random key of at least 128 bits that the document knows the
SHA-256 of, and that carries a role. The slug already works this way for
reading; a link grant is the same idea with a role attached, and it exists
because of who the readers are. Coauthors have GitHub accounts. The
reviewers of a paper often do not, and the reviewer of a blind submission
must not be named at all. A commenter by link types a display name, the way
an anonymous commenter does today, and the server records the link the
comment came in on, so the owner can tell reviewer two's comments from
reviewer three's without either having signed anything.

The key travels in the URL fragment, `/docs/<slug>#k=<key>`, which a browser
never sends to a server, so it lands in nobody's access log or `Referer`. The
reader page takes it from the fragment, keeps it in `localStorage` under the
slug, and presents it on every request for that document: as a header on
`fetch`, as a query parameter on the socket, which is the one place a browser
cannot set a header. The visible URL is cleaned once the key is stored, and
"Copy link" in the reader puts it back, so a link copied from the bar is the
link that was shared. Over TLS the socket's query string is seen by the
server and nobody else; it is logged as the hash, never the key.

**What a link may not grant.** A link cannot make an editor unless the
server lets anyone publish, because an edit made under a link has no name
behind it and the history would record `by: nobody`. This falls out of the
ceiling rule below rather than being a rule of its own.

## Visibility, which is a different axis

Who may read is not a role somebody holds but a property of the document:

| visibility | who may read |
| --- | --- |
| `link` | anyone with the link; the default, and today's behaviour |
| `private` | people named on the document, in any role |
| `listed` | anyone with the link, and the document appears on the landing page for everyone |

Discoverability and access are different questions and are kept as
different values rather than as a flag each, because the combinations that
two flags allow and this table does not -- a private document that is
listed -- mean nothing. `listed` is what a public instance uses for a
document its owner wants found; an operator who wants no public listing at
all passes `--no-listing`, and `listed` behaves as `link`.

The landing page's `visible` filter becomes: the examples, everything the
caller holds a role on, and everything `listed`. A document shared with you
by name is in your list; one shared by link is not, because the link is in
your browser and not on your account, and it is in that browser's list
instead, from `localStorage`.

### Reading a private document

Today a document is read from the documents origin, `/raw/<slug>/<sha>.html`,
which by design shares nothing with the origin the cookies live on: a
`__Host-` cookie never crosses hosts. There is therefore no identity on the
documents origin and no way for it to enforce `private`, short of the main
origin minting a short-lived signed URL for each read, which is what
`--s3-direct-reads` already does with presigned URLs. It could be built
that way.

It is not, because `01-SPEC-history.md` removes the need. Under that spec the
frame on the documents origin is an empty shell; the text arrives through
the socket on the main origin, where the identity is, and is pushed into the
frame by `postMessage`. Checkpoints come from `history/<slug>/…` on the main
origin. Every read a document has passes one check on one origin, and
`private` is that check. Private documents therefore ship after that spec,
not before, and this spec's one change to it is that the "anyone who may
read the document" in its storage table means the reader role as defined
here.

## The server's switches become ceilings

`--publishers` still says who may create a document. Beyond that, both
switches become the widest a document may open the corresponding role, and
a document may only be stricter:

- A grant of **editor**, by name or by link, must be allowed by
  `--publishers`, because an editor puts content on the server, which is
  what that switch governs. On `--publishers alice,bob`, alice may name bob
  and nobody else. On `any`, any account. On `anyone`, a link may edit.
- A grant of **commenter** must be allowed by `--commenters`, in the same
  way. A document may close comments to named people on a server whose
  switch says `anyone`; it may not open them to anyone on a server that
  names its commenters.
- **reader** has no switch, because reading has never had one. `private`
  is the document closing what the server left open.

An operator's policy is never loosened by a sharing dialog, and the dialog
shows only the choices the switches allow.

## Storage

On the index entry, which every request already consults to answer
ownership:

```json
{
  "visibility": "link",
  "editors":    [{"id": "9124", "login": "annegrandchamp", "since": "2026-09-05T14:02:11Z"}],
  "commenters": [{"id": "3310", "login": "rmcelreath", "since": "2026-09-05T14:04:50Z"}],
  "links": [
    {"hash": "c1f4…", "role": "commenter", "label": "reviewer 2",
     "since": "2026-09-05T14:10:00Z", "until": "2027-03-05T14:10:00Z"}
  ]
}
```

`publisher` and `publisher_id` stay as they are and mean owner. A few ids
and a few hashes per document do not change what loading the index costs.
Revoking is deleting the row; a link's key is shown once, at creation, and
never again, since only its hash is kept. `until` is an expiry; a link past
it answers as no link at all, and the dialog offers to renew, which is a new
row and a new key.

On a `Comment`, one field: `via`, the hash of the link the comment came in
on, empty for a named commenter. It is what lets the owner group a blind
reviewer's comments, and it is not exported, since the export is the
reviewer's words and not the mechanics of how they arrived.

`destroy` deletes all of it with the entry, as it deletes everything else.

### Visitors

A visitor -- a browser with no sign-in, on a server that allows it -- owns
what it uploads and can share it by link, but cannot name anyone, since a
name is matched against a GitHub id the visitor does not have. When a
visitor signs in, the documents its visitor key owns are adopted by the
account: `publisher` is rewritten to the login, `publisher_id` set, and the
quota moves with them. A document with no publisher at all, on a deployment
that records none, is not a visitor's and stays publisher-less, as the rules
require. This is also the answer to "I cleared my cookies and my documents
are gone", which the README today can only warn about.

## What it looks like

### Follow-up: scoped credentials for automation

After named and link grants, an owner may mint a revocable bearer token
scoped to one document and a role. It is presented on the WebSocket upgrade
or an HTTP request, without a browser cookie. Token authentication feeds the
same role function and server-policy ceilings as other grants; it must not
be mistaken for the existing GitHub bearer token. An editor token cannot
bypass the publisher policy: where a named account is required, it must be
bound to an allowed account rather than treated as anonymous editing.

Store only the token's hash and its scope, role, label, and rate budget.
The token supplies a distinct author key so automated comments can be
attributed to the automation client rather than its issuer. Its rate budget
is separate from a human's. Revocation must stop further reads and writes,
including on an already connected socket. The client cannot claim its own
identity or role in a message.

This is follow-up work, not part of the completed role-function step.
Tests must cover document isolation, forbidden roles, attribution, rate
limits, and revocation on both HTTP and an open socket. The peer client and
snapshot interface are specified in `04-SPEC-sync.md`.

### In the reader

The copy-link button at the right of the nav becomes **Share**, and copying
the link is the first thing in it. The dialog has three parts, top to
bottom:

1. **Visibility.** Three radio buttons, `Anyone with the link`, `Only people
   named below`, `Anyone, and listed on the front page`; the last absent
   under `--no-listing`.
2. **People.** A list of names with a role beside each, and a field to add
   a GitHub login. Roles offered are the ones the server's switches allow.
   The owner is shown first and cannot be removed here; `Transfer` is behind
   a confirmation, as `destroy` is.
3. **Links.** One row per link: its label, role, expiry, `Copy` and
   `Revoke`; and a button to make a new one, asking for a role and a label.
   The key is shown once, in the row, with the copy button, and the row
   says so.

A reader who arrived by link sees no Share button. A commenter or editor by
name sees a Share button that opens to the same dialog read-only, so they
can see who else is in the room; only the owner edits it.

The document's own endpoint, which today answers `can_moderate`, answers
`role` instead, and the reader derives every affordance from it: the editor
is offered at `editor` and above, the comment tools at `commenter` and
above, Share at `owner`.

### On the command line

```sh
komodoc share c9k                              # print visibility, people, links
komodoc share c9k --editor annegrandchamp      # by name
komodoc share c9k --commenter rmcelreath
komodoc share c9k --link commenter --label "reviewer 2" --until 180d
komodoc share c9k --visibility private
komodoc share c9k --revoke annegrandchamp      # a login, or the first characters of a link hash
komodoc transfer c9k alice
```

`komodoc list` gains the documents shared with the caller by name, marked
with the role. The `--link` command prints the full URL with the key in the
fragment, once, and says it will not be shown again.

## What it is not

It is not a hiding place for comments. Every reader sees every comment, as
today. A blind review in which reviewers cannot see one another's comments
until the owner reveals them is a fifth kind of thing -- a visibility on
comments rather than on documents -- and is its own spec, which this one
makes possible by recording `via`.

It is not a second identity provider. A grant by name is a grant to a
GitHub account, because that is the only account the server knows. If that
is too narrow for the reviewers a journal has, the fix is another provider,
which is a change to the auth module and not a change here; link grants carry the
load meanwhile, and were designed to.

It is not a permission system for the server. `--publishers` and
`--commenters` stay the operator's, and nothing a document says can widen
them.

It is not delegation. An editor cannot share. Google Docs lets editors
invite by default and the surprises it causes are well known; the owner is
the one whose quota and whose name are on the document, and the one who
shares it.

## Steps

1. **The role function.** `role_of(entry, identity, key) -> Role`, replacing
   `owned_by` and `can_moderate` at every call site with no change in
   behaviour: owner where `owned_by` was true, commenter where the server's
   switch allows, reader otherwise. The document endpoint answers `role`.
   Tests: the existing ownership tests pass through the new function.
2. **Grants by name.** The `editors` and `commenters` fields, the ceiling
   check against the switches, `komodoc share` by login, `--revoke`,
   `transfer`. The `y-*` gate in the socket handler asks for `editor` rather
   than owner; `apply_from` asks for `commenter` instead of the switch. `list`
   shows what is shared. Tests: an editor edits and a commenter cannot; a
   grant the switch forbids is refused with a message naming the switch; a
   transfer moves the quota.
3. **Grants by link.** The `links` field, the key in the fragment, the
   header and the query parameter, hashing, expiry, `via` on comments,
   `--link` and `--label` and `--until`. Tests: a link comments and cannot
   edit; a revoked link reads as no link; an expired link likewise; a link
   editor is refused unless publishers is `anyone`.
4. **The dialog.** Share in place of the copy-link button, the three
   sections, read-only for non-owners.
5. **Visitor adoption.** On the OAuth callback, rewrite the visitor's
   entries to the account. Test: upload anonymously, sign in, the document
   is listed and owned.
6. **Visibility.** After `01-SPEC-history.md` steps 1 to 3. `private` enforced
   at `y-open` and the history routes; `listed` and `--no-listing`; the
   landing filter. Tests: a private document answers 404 to a stranger, as
   an unowned slug does today, and hello to a named reader.

Steps 1 and 2 are the foundation and a couple of days. 3 is a day or two,
most of it the key's path through the browser. 4 is a day. 5 is an
afternoon. 6 waits on the history spec and is a day once that is in.

## Open questions

- **Should links expire by default?** A round of review has an end; a link
  that lives forever is a leak waiting for a forwarded email. Six months,
  renewable from the dialog, is the proposal, and named grants do not
  expire. An operator may want `--link-lifetime` to set the ceiling.
- **A 404 or a 403 for a private document?** A stranger at a private
  document's URL learns nothing from a 404 and something from a 403. The
  store today answers 404 for a document that is somebody else's to
  replace, on the same reasoning, and this spec follows it; but a named
  reader who has not signed in needs to be told to, which a bare 404 does
  not do. The reader page can answer that: a 404 from the API on a slug the
  page was opened at shows "sign in, if this was shared with you".
- **Editors and pinning.** `01-SPEC-history.md` gives pinning to the owner.
  This spec gives it to editors, since it is a decision about the text and
  not about the document's ownership; if that is wrong, it is one row of the
  table.
- **Where the key lives in the browser.** `localStorage` under the slug
  means a key is per browser, and a reader who opens the link on their
  phone pastes it again, which is the right behaviour for a secret. A
  cookie scoped to the path would follow the browser without the page's
  help and be cleared with the others; it is the fallback if `localStorage`
  proves awkward with the frame.
- **Rate limits by link.** Comments are rate-limited by address today. A
  link is a better key for a commenter who arrived by one, and a worse one
  for a link forwarded to a department; left as address until it hurts.

## What was built

Step 1 only, on the existing Rust host.

`IndexEntry::role_of(owner_key, caller_id, may_comment)` in `store.rs`
answers the highest role a caller holds, and `Role` is the ladder this spec
names -- reader, commenter, editor, owner -- with `at_least` for the question
every gate really asks. The document endpoint answers `role` beside the
`can_edit` and `can_moderate` it already answered, and `Reader.svelte` derives
the editor from `role` when it is there and from `can_edit` when it is not, so
a cached page still works. The server's `--commenters` switch is the ceiling
it will be: nobody is offered `commenter` that the switch does not allow.
The gates that mean "may edit" ask for the editor rung through it -- the
`y-*` messages on the socket, the moderation of anyone else's comment, and the
source endpoint -- while the two places that ask about ownership itself, the
landing-page filter and the slug a publish may replace, still ask `owned_by`,
which is the question they mean. Behaviour is unchanged, which is what step 1
asked for; the test is that the existing ownership suite passes through the new
function, plus one that pins the three answers a stranger, a commenter and an
owner get.

Nothing of steps 2 to 6 is built: no `editors` or `commenters` on the entry,
no link grants, no `komodoc share`, no dialog, no visitor adoption, no
visibility. `Role::Editor` is therefore a rung nobody stands on, and is marked
as such in the code.
