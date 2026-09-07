# SPEC: the catalogue as a transactional database

## Why

The store keeps two kinds of data and treats them as one. Content is large,
immutable and content-addressed: sources, checkpoint trees, text blobs,
figures, renderings. Object storage is the right home for it, nothing there
is ever rewritten, and it already scales. The catalogue is the opposite:
small records that change, that must be found by slug, by owner and by the
accounts named on them, and that carry the sums admission is decided on.

Today the catalogue is one object, `index.json`, held whole in memory and
rewritten whole on every change. Every publish, rename, grant, guest,
deletion and checkpoint serialises every record in the deployment and PUTs
them as one body under a compare-and-swap. The cost of each write grows with
the number of documents, every write in the deployment serialises on one
object, a lookup that misses reloads the whole index, and a listing for one
account walks every record. That is the right design for a few hundred
documents and the wrong one past a few thousand.

This spec moves the catalogue into a transactional SQL database and leaves
content exactly where it is. Nothing has been released, so there is no
migration: the new layout is the only layout, and the compatibility paths
that read older ones are removed.

## What moves and what stays

The database holds what `IndexEntry` holds now: the document, its owner, the
accounts named on it, the links that carry a role, the accounts that came in
through a link, and the sizes quotas are summed from. It holds one thing the
index did not: which documents each signed-in account has commented on,
so that an account can be erased without reading every room in the bucket.
Nothing else moves.

The bucket keeps, unchanged, everything keyed under a slug: `sessions/`,
`history/`, `assets/`, `renderings/`, `rooms/`, `chat/`, `examples/`. Each of
those is one document's own state, written by the room that holds the
document open, guarded by its own compare-and-swap, and never queried across
documents. `session.key` also stays in the bucket; it is not catalogue data
and moving it would gain nothing.

The `documents/` and `sources/` prefixes, `legacy_source_key`,
`migrate_legacy_source`, and the case in `history.rs` that reads a checkpoint
without a tree as a tree of one file all exist to read what earlier builds
wrote. Nothing has been written, so they go. So do the serde defaults on
`IndexEntry`, `Grant` and `LinkGrant` that describe what a field absent from
an older index reads back as.

## One dialect, two places

The database is SQLite in both deployment shapes, so there is one schema and
one set of statements, and the test suite that runs against an in-memory
database exercises the SQL production runs.

- **A directory.** The catalogue is a file, `catalog.db`, beside the blobs.
  This is what running Komodoc on your own machine means, and needs nothing
  installed.
- **A bucket.** The catalogue is a hosted SQLite-dialect database reached
  over the network, so the VPS still holds no durable state of its own.
  Turso is the reference host: its free plan is ample for this shape, its
  row-write budget is two orders of magnitude above Cloudflare D1's, and the
  `libsql` crate opens a local file and a remote database through the same
  connection API. D1 stays possible later through its HTTP API and would
  need its own driver, not its own schema.

Both go through the `libsql` crate. The SQL is kept to the subset SQLite
shares with Postgres: no `WITHOUT ROWID`, no JSON operators, no
`INSERT ... ON CONFLICT` beyond the plain form, no SQLite-only pragmas in
the schema itself. A Postgres implementation later is then a port of the
driver layer and of nothing else.

### Configuration

`--catalog` names the database: a path, or a `libsql://` URL with the token
in `KOMODOC_CATALOG_TOKEN`. Tokens come from the environment for the same
reason bucket credentials do: a flag lands in the process table.

When blobs are a directory and `--catalog` is absent, the catalogue is
`<dir>/catalog.db`. When blobs are a bucket, `--catalog` is required: a
bucket deployment that silently opened a local file would hold its ownership
records on the one disk the design promises nothing lives on, and losing the
VPS would then lose every grant. `serve` prints where the catalogue is on
the same startup line that says where the blobs are.

The schema is created and later changed by the migrations described below,
which the server applies at startup.

## Schema

```sql
CREATE TABLE documents (
    slug            TEXT PRIMARY KEY,
    title           TEXT NOT NULL,
    sha             TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    published_at    TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    example         INTEGER NOT NULL DEFAULT 0,
    owner_key       TEXT NOT NULL,
    owner_id        TEXT NOT NULL,
    owner_name      TEXT NOT NULL,
    size            INTEGER NOT NULL,
    source_format   TEXT NOT NULL,
    main            TEXT NOT NULL
);
CREATE INDEX documents_owner ON documents (owner_key);
CREATE INDEX documents_owner_id ON documents (owner_id);

CREATE TABLE grants (
    slug        TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    role        TEXT NOT NULL,          -- 'editor' | 'commenter'
    account_id  TEXT NOT NULL,
    handle      TEXT NOT NULL,
    name        TEXT NOT NULL,
    since       TEXT NOT NULL,
    PRIMARY KEY (slug, role, account_id)
);
CREATE INDEX grants_account ON grants (account_id);

CREATE TABLE links (
    slug    TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    role    TEXT NOT NULL,              -- one link per role per document
    hash    TEXT NOT NULL,
    sealed  BLOB NOT NULL,              -- the key, encrypted; see below
    label   TEXT NOT NULL,
    budget  INTEGER,
    since   TEXT NOT NULL,
    until   TEXT NOT NULL,
    PRIMARY KEY (slug, role)
);
CREATE UNIQUE INDEX links_hash ON links (hash);

CREATE TABLE guests (
    slug        TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    account_id  TEXT NOT NULL,
    name        TEXT NOT NULL,
    since       TEXT NOT NULL,
    link_hash   TEXT NOT NULL,
    PRIMARY KEY (slug, account_id)
);
CREATE INDEX guests_account ON guests (account_id);

CREATE TABLE usage (
    owner_key   TEXT PRIMARY KEY,       -- '' is the deployment total
    bytes       INTEGER NOT NULL,
    documents   INTEGER NOT NULL
);

CREATE TABLE authorship (
    author_key  TEXT NOT NULL,          -- github:<login>, google:<sub>
    slug        TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    PRIMARY KEY (author_key, slug)
);
```

Three names change on the way in, and only names. `publisher`,
`publisher_id` and `publisher_name` become `owner_key`, `owner_id` and
`owner_name`, which is what they are. A grant's `login` becomes `handle`,
which is what its doc comment already calls it; the field was only ever
named `login` for the sake of indexes already written, and there are none.

One field is added. `published_at` is when the document's bytes were last
uploaded, and is what the uploads-per-hour limit counts. `updated_at` is
when the newest checkpoint landed, and is what listings sort by and what
retention measured from `updated` reads. Today the two are one column, and
the hourly upload limit counts checkpoints as uploads because of it.

`usage` is the sum of `size` and the count of rows per owner, plus a row for
the whole deployment. It is maintained in the same transaction as every
change to `size` or ownership, so admission reads two rows instead of
summing the table. It is derivable, and `seed` and a recount command can
rebuild it from `documents`.

`authorship` records that a signed-in account has commented on a document.
The room writes one row the first time a comment or reply from that
account lands, keyed the way `Comment::author` already keys it, and never
again for that pair. Comments themselves stay in `rooms/`; this table is
what makes "every document this account has written on" one query rather
than a read of every room in the bucket. A grant or a guest row is not a
substitute: a guest is pruned when the link that let them in is rotated,
and a public example needs no grant at all, so neither says where an
account's comments are.

## Migrations

Nothing is released, so there is nothing to migrate from today. There will
be, and a schema with no migration story is one that gets changed by hand
on a live database. The policy is fixed now so the first release ships with
it.

**Numbered, embedded, forward only.** A migration is a file under
`crates/komodoc/migrations/`, named `NNNN_what_it_does.sql`, compiled into
the binary and applied in order. The schema above is `0001_catalog.sql`.
A fresh database is an empty one with every migration applied, so there is
no separate schema file to keep in step with the migrations, and a
database created on the day of release and one created a year later are
the same rows arrived at the same way. There are no down migrations.
Undoing one is restoring the backup taken before it, which is the only
undo that is ever actually tested.

**Versioned by `PRAGMA user_version`.** The database records the number of
the last migration applied. The binary knows the highest number it ships.
At startup the server compares them:

- Database behind: apply each pending migration, in order, each in its
  own transaction. SQLite's DDL is transactional, so a migration that fails
  leaves the version where it was and the server refuses to start, with the
  migration's name and the error.
- Database level: start.
- Database ahead: refuse to start. An older binary must never run against
  a shape it does not know. The message names both versions.

The check and the apply happen inside one `BEGIN IMMEDIATE` transaction, so
two servers starting together against one hosted catalogue cannot both
apply the same migration; the second finds the version already moved.
With embedded replicas the transaction runs at the primary, which is where
DDL must run in any case.

**Code when SQL is not enough.** Some changes need the program, such as
re-sealing every link key under a new derivation. Such a migration is a
Rust function registered in the same numbered sequence, run in the same
transaction discipline, and named the same way. The sequence is one list
whichever kind each entry is.

**Before applying.** On a directory deployment the server copies
`catalog.db` to `catalog.db.before-NNNN` before applying migration NNNN,
and says so. On a hosted deployment the host's point-in-time restore is the
backup, and the startup line says which version it is moving from and to
so the moment is findable. `komodoc migrate --catalog <where>` applies
pending migrations without starting a server, and `--dry-run` lists them,
for an operator who wants to migrate first and swap binaries second.

**Deploying a schema change.** Stop the old server, start the new one, and
let it migrate. A running older server that sees a newer schema arrive
through replica sync is not a case this policy handles, and the single-VPS
shape does not produce it. If there are ever two servers, the order is:
migrate with the new binary once, then restart each.

**Tests.** Every migration ships with a test that builds a database at the
previous version with representative rows, applies the migration, and
checks the rows come through. A further test applies all migrations from
empty and asserts the version matches the highest the binary ships, so a
file added without being registered fails before it is released.

**Compatibility rule.** A migration may add, rename or drop anything, since
the binary that ships it is the only one that will run against the result.
What a migration may not do is depend on data the previous version did not
guarantee: a column the old binary allowed to be empty is migrated with an
explicit rule for empty, written in the migration, not left to a default.

## Link keys at rest

Everything else in these tables has to be readable to do its job. The link
key does not: it is read only when an owner asks to copy a link again, and
in every other request the caller presents it and the server matches the
hash. So it is the one column stored encrypted, and a copy of the catalogue
on its own then holds no usable bearer credential. An attacker needs the
bucket as well, where `session.key` lives.

The sealing key is derived from `session.key` with HKDF-SHA256 under a
fixed label, so the cookie-signing key and the link-sealing key are
different bytes with one secret behind them. Each row is sealed with an
AEAD, a fresh random nonce stored as the prefix of `sealed`, and the link's
hash as associated data, so a ciphertext moved to another row does not open.
A small pure-Rust AEAD crate such as `chacha20poly1305` is the whole
dependency.

`session.key` is minted once on an empty deployment and never rotated by
the server, which is what makes this safe to key on. Replacing it by hand
signs every reader out and leaves every sealed link unreadable; a link that
cannot be unsealed is shown as one that must be reset, which is the same
answer an expired link gives.

`IndexEntry` goes. The server works with a `Document` that carries the
row's fields and, when a caller's role has to be decided, the grants, links
and guests loaded beside it. `role_of`, `owned_by`, `link_role` and the
rest of the authorisation logic move onto that type unchanged in meaning.
Nothing is read into a struct in order to be rewritten whole: every change
is a statement against the rows it touches.

## The trait

The server, the rooms, the janitor and the seed command see a `Catalog`
trait, as they see `BlobStore` today, and never a connection. `Store`, the
type that held the index in memory, is deleted. The content methods it
carried (`read_source`, `drop_derived`) become functions over `BlobStore`,
which is what they always were.

The trait has one operation per thing the product does to the catalogue.
There is no `modify` taking a closure, because a closure over a whole entry
is the one-object design in a different coat: it reads everything to write
anything, and it cannot be expressed to a remote database as the statement
it actually is.

- `get(slug)` reads one document with its grants, links and guests.
- `create(Publication)` inserts a document under admission.
- `replace(slug, Publication)` swaps its bytes and size under admission.
- `rename(slug, title)`.
- `record_checkpoint(slug, sha, size, format, main)`.
- `adopt(slug, owner)` moves a visitor's document to a signed-in account.
- `grant(slug, role, account)` and `revoke(slug, role, account_id)`.
- `set_link(slug, Link)`, which rotates the role's link and prunes the
  guests it let in, and `drop_link(slug, role)`, which prunes the same.
- `pin_guest(slug, Guest)`.
- `remove(slug)`.
- `note_author(slug, author_key)`, called by the room on the first comment
  or reply from a signed-in account on that document.
- `visible(caller)`: the documents a caller owns, is named on, or came
  into as a guest, plus the examples when listing is on, each row carrying
  the caller's role. One query over the indexes above.
- `footprint(account)`: everything the catalogue holds for one account,
  for the erasure below: the slugs it owns, the slugs it is named on or a
  guest of, and the slugs it has written on.
- `forget(account)`: deletes the account's grants, guests, authorship and
  usage rows in one transaction. Owned documents are removed one by one
  through `remove`, since each has bytes to delete first.
- `expired(before, from)`: the slugs older than a cutoff by `created_at`
  or `updated_at`, for the janitor.
- `all()`: every slug, for the seed command and the orphan sweep only.
- `room_for(slug)`: two rows of `usage`.

Each is one transaction. The call sites in `server.rs` that today read an
entry, edit it in a closure and save it become calls to the operation they
were performing. That is a change at each call site and a simplification
at each.

Everything that existed for the one-object design goes: `StoreState`,
`index_version`, `save_locked`, `reload_locked`, `refresh_locked`,
`refresh`, `REFRESH_EVERY`, `load_index`, `INDEX_KEY`, and `swap` on the
index. `BlobStore::swap` stays, since rooms, manifests and mailboxes still
use it.

## Admission

`create` and `replace` decide admission inside the transaction that inserts
or replaces the row. It reads the owner's `usage` row and the deployment's, counts the
owner's `documents` rows with `published_at` inside the last hour, applies
the four ceilings from `StorageLimit` exactly as `admit` does today, and on
success writes the row and adjusts both `usage` rows in the same
transaction. Two publishes racing for the last megabyte cannot both be
admitted: the database serialises them, and the second reads the first's
sums. That is the guarantee the compare-and-swap retry loop provides today,
without the retry.

`record_checkpoint` updates `size`, `sha`, `source_format`, `main` and
`updated_at` in one statement and adjusts `usage` by the size delta. It is
one row write per checkpoint, beside the several bucket writes a checkpoint
already makes, and it is the write that sets the pace of catalogue traffic
on a hosted database. Turso's free budget absorbs it comfortably; D1's
daily budget would not at any real activity, which is why Turso is the
reference host.

## Two stores, one order

There is no transaction across the bucket and the database. The ordering
that keeps the half-states safe is the one the store already follows.

- **Create or replace.** Bytes first, row last. A row that names a
  checkpoint the bucket has is the only state a reader can reach; bytes the
  catalogue never named are unreachable, which is the half-state to prefer.
- **Delete.** Purge the room, delete the document's bucket prefixes, delete
  the row last. Until the row goes the document is still listed, which is a
  better half-state than a listing that points at nothing.
- **Checkpoint.** The manifest and objects are written by the room, then
  the row is updated. A row behind the manifest is a document whose listing
  is a checkpoint old, and the next checkpoint corrects it.

Orphans in the bucket, from a create that wrote bytes and then failed to
write its row, are reclaimed by the janitor: list the slug prefixes in the
bucket, drop those with no row, on the same timer as expiry. A row whose
bytes are missing is a fault to report, not to repair, exactly as an
unreadable index is today.

## The read path

Authorisation runs on every request and cannot wait on a network round
trip. The old design answered that with the whole index in memory and a
best-effort convergence between instances. This one writes no cache.

On a directory deployment the database is a local file, and a lookup by
primary key is a memory read in all but name. On a bucket deployment the
server opens the remote database as a Turso embedded replica: a local file
that mirrors the remote, serves every read from disk, and forwards every
write to the remote, which syncs the result back. Reads are local, writes
are durable at the host, and the file on the VPS is a replica that can be
thrown away and rebuilt from the remote. Two servers on one catalogue each
hold a replica and see each other's writes on the next sync, which is the
shape of today's convergence with none of today's code for it.

The sync interval is a flag with a short default. A replica that cannot
reach the remote keeps answering reads and fails writes, which is the
correct half-state: nobody is signed out or refused a document because the
host is briefly away, and nothing is admitted against sums the host has not
confirmed.

## Erasing an account

A signed-in user can remove everything Komodoc holds that is theirs, and an
operator can do the same for them on request. Both reach one server
operation, `erase_account`, which takes the account's id and author key
and does the following, in this order, each step idempotent so that a run
that fails partway is simply run again.

1. **Comments elsewhere.** For every slug in the account's `authorship`
   rows, open the room and delete every comment and every reply whose
   `author` is the account. A deleted comment takes the replies under it,
   whoever wrote them; a thread cannot stand on a remark that is gone. The
   room saves and broadcasts as it does for any deletion, so open browsers
   see it happen.
2. **Membership.** `forget(account)` drops the account's grants, guest
   rows, authorship rows and usage row. Share dialogs on those documents
   stop naming the account on their next load.
3. **Owned documents.** For every slug the account owns, the ordinary
   delete: purge the room, delete the document's bucket prefixes, delete
   the row last. This takes the document's comments by everyone, its
   history, its assets, its renderings and its chat mailboxes with it,
   because they are the document's.
4. **The session.** The response clears the cookie. Session cookies are
   signed and stateless, so an old cookie remains a valid identity until it
   expires; there is nothing behind it any more, and the next sign-in
   starts from nothing.

What this does not remove is said plainly in the confirmation. Edits the
account made to other people's documents are in those documents' text and
checkpoints, which belong to their owners, and are not attributable in the
CRDT in any case. Comments the account made while not signed in, or through
an automation link, are keyed on a visitor or link pseudonym and cannot be
tied to the account. Backups at the catalogue's host and any versioning on
the bucket retain what they retain for their own windows.

**Self-service.** The account menu offers it. The dialog shows what will
go, from `footprint`: how many documents the account owns, how many it
is named on, and how many it has commented on, with the titles of the
owned ones. It asks the user to type their handle to confirm. The request
is refused for a caller who is not signed in, since a visitor has no
account to erase and a link names nobody.

**Operator.** `komodoc erase --account <handle>` runs the same operation
against the deployment's catalogue and bucket, for a request that arrived
by email rather than through the app. It prints the footprint and asks for
the handle again before proceeding, and reports what was removed and what
was left, by count.

## Seeding and tests

`seed` clears the catalogue tables and the bucket keys together, then
publishes the examples through `create` like any other caller.
`clear_storage` stays for the bucket half.

Unit tests open an in-memory database and a temporary directory, as they
open `FsStore` in a temporary directory now. One test module runs the same
suite against a remote database when `KOMODOC_TEST_CATALOG` names one, and
is skipped otherwise, so the remote driver is exercised before a release
without being on the path of every `cargo test`.

## Deploy

`make deploy` gains two steps: create the database on the host if it does
not exist, and hand the server its URL and token beside the bucket
credentials; then, on a release that carries a migration, stop the old
server before the new one starts, so the migration runs once against a
quiet catalogue. Backups of the catalogue are the host's; on a directory
deployment they are the operator's, and `catalog.db` is one file to copy
while the server is stopped, or through SQLite's online backup while it
runs.

## Out of scope

Rooms, comments, history manifests, chat mailboxes and rendering
provenance stay in the bucket; none is queried across documents and each
has its own compare-and-swap already. Comments are the one of these that a
later feature might want across documents, such as everything unresolved
on the documents a reviewer is named on. That is a product question, and
the day it is asked the answer is a `comments` table in this catalogue, not
a scan of `rooms/`. A Postgres implementation of the trait is deliberately
kept possible and deliberately not written. Billing, organisations and
search are the features that would call for it.
