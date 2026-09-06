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

Each role's link may carry an owner's label and a comments-per-hour budget.
The label is a memo -- "CI" or "reviewer" -- rather than another identity or
another link, survives rotation when no replacement label is given, and is
dropped with the link on revoke. A caller presenting a live link is rate
limited by that link's hash rather than its network address, so every machine
or person sharing the link also shares its budget. An absent budget uses the
deployment's ordinary comment limit. Exhausting the budget stops comment
actions but never reading.

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
