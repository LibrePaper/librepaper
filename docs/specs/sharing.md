# SPEC: sharing, and who may do what to a document

Status: the sharing model is built, and it is links and nothing else. A
document has four ways in. The owner's is the bare URL, which opens for the
owner's sign-in and for nobody else. The other three are links the owner
mints, one per role -- reader, commenter, editor -- each with an expiry,
each rotated by minting it again and killed by revoking it. There is no
visibility setting: the URL alone is not a link, a stranger holding it is
answered as a deleted document answers, and the reserved examples are the one
kind of document open to all. `komodoc publish` mints the read link with the
document and prints it, with no expiry, so what it prints is the thing to
send. A read link is read-only whatever `--commenters` says; that switch is a
ceiling on what a link may carry, and grants the commenter rung to nobody but
readers of an example. An edit link authorizes and the signed-in account
behind it attributes wherever `--publishers` requires a name; anonymous
commenters get a stable per-document pseudonym (`pseudonym.rs`); a signed-in
reader who arrives by a link is a guest of the document until that link is
rotated or revoked. Named grants by login are legacy, honoured and revocable
but never created. `IndexEntry::role_of` and `readable_by` in `store.rs` are
the whole of the decision, and every route asks them.

The documents origin, which serves an HTML document's page into the frame so
its scripts run, holds no identity. The reader fetches a token from
`/api/documents/<slug>/frame` on the origin that does -- signed for the slug,
good for two minutes, minted only for a caller `may_read` admits -- and puts
it on the frame's URL; without one the origin serves the empty shell.

The command line presents a link with `--key`, taking the key or the whole
link, on `comment`, `edit`, `sync`, `history` and `export`, as the
`x-komodoc-key` header on every request and on the socket upgrade; with a
key and no sign-in the caller is whoever the link says, and with both the
link authorizes and the account attributes, as in the browser.

What follows is what is left.

## A machine's link

A script or a bot holds a comment or edit link, the same credential a person
pastes into a browser, and `--key` is how it presents one. Two things would
make a link fit a machine caller specifically, rather than whichever person a
human happened to forward it to.

**A label.** `LinkGrant.label` in `store.rs` exists and is never written; it
is marked legacy. Make it live: the share change accepts `label` inside
`link` beside `role` and `until`, `komodoc share --link <role>` takes
`--label`, `format_role_row` prints it, and the dialog gets a label input
beside the expiry and shows it on the row. A label survives a rotation of the
same role and is dropped on revoke. A document holds one link per role, so a
label tells the roles' links apart by what they are for -- "CI" on the edit
link, "reviewer" on the comment link -- and is a memo, not a way to have two
comment links; several links per role is a different model and is not
proposed.

**A rate budget per link.** `Room::rate_ok` keys on the caller's address
alone. When a request carries a link key, the rate key becomes the link's
hash instead, so a link forwarded to a department shares one budget and a
script has its own rather than borrowing its host's. A `budget` on
`LinkGrant`, comments per hour, empty for the address limit, set through the
same share change and CLI flag as the label. Tests: two addresses on one
link share a budget, the budget resets on the hour, and a link past its
budget still reads.

## A front page

`--no-listing` exists and today it hides the reserved examples from anyone
who holds nothing on them; nothing else was ever listed to strangers. If a
front page for other people's documents is wanted, it is a per-document
opt-in that publishes the read link -- a checkbox in the share pane, shown
only where the deployment allows listing -- and a route that answers without
a publisher. It is not a third kind of access, since reading is a link's to
give and the front page would hand that link out.

## Open questions still open

- **A `--link-lifetime` ceiling.** Links expire at six months by default,
  `--until never` is allowed for an owner who wants none, and the read link
  `publish` mints has none. No operator ceiling was added; there is nothing
  yet asking for one.
- **The read link `publish` mints.** It is the least a link can be, which is
  why it is the one minted unasked. A deployment whose whole purpose is
  collecting comments may want `publish` to mint the comment link instead,
  or as well; the hint `publish` prints says how to mint one, and that is the
  answer until somebody asks for the other.

## What it is not

It is not a hiding place for comments. Every reader sees every comment. A
blind review in which reviewers cannot see one another's comments until the
owner reveals them is a visibility on comments rather than on documents, and
is its own spec -- which `via` on each comment already makes possible.

It is not a second identity provider. Coauthors and reviewers are named with
a link, never by a handle the server would have to resolve.

It is not a permission system for the server, and it is not delegation: an
editor cannot share, and the owner's own way in is a sign-in rather than a
link, so there is no full-power bearer to leak.
