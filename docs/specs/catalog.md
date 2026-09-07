# SPEC: the catalogue as a transactional database

## Why

The store keeps two kinds of data and treats them as one. Content is large,
immutable and content-addressed: sources, checkpoint trees, text blobs,
figures, compiled PDFs. Object storage is the right home for it, nothing
there is ever rewritten, and it already scales. The catalogue is the
opposite: small records that change, that must be found by slug, by owner
and by the accounts named on them, and that carry the sums admission is
decided on.

Today the catalogue is one object, `index.json`, held whole in memory and
rewritten whole on every change. Every publish, rename, grant, guest,
deletion and checkpoint serialises every record in the deployment and PUTs
them as one body under a compare-and-swap. The cost of each write grows
with the number of documents, every write in the deployment serialises on
one object, a lookup that misses reloads the whole index, and a listing
for one account walks every record. The same pattern repeats per document
in the bucket: the comment list, the checkpoint manifest and the chat
mailbox are each one JSON object rewritten whole under compare-and-swap on
every change. That is the right design for a few hundred documents and the
wrong one past a few thousand.

This spec moves the catalogue and every other mutable record into a
transactional SQL database and leaves bytes exactly where they are. It also
fixes the write budget, since with two metered stores the bill is decided
by design choices rather than by traffic alone. Nothing has been released,
so there is no migration from the current layout: the new layout is the
only layout, and the compatibility paths that read older ones are removed.

## What moves and what stays

The database holds durable records that change or are queried:

- **Documents**, with their owner, size and main file: what `IndexEntry`
  holds now.
- **Grants, links and guests**: who is named on a document, the links that
  carry a role, and the accounts that came in through a link.
- **Accounts**: one row per signed-in identity. There is none today; the
  handle and display name are copied onto every document and grant, and
  the retention policy's contact address and paid tier have nowhere to
  live. This is the table that gives them a home and gives a rename one
  place to land.
- **Comments and replies**: the whole content of `rooms/<slug>.json`.
- **Checkpoints**: the entries of `history/<slug>/index.json`, one row
  each. The trees they name stay in the bucket; see the write budget.
- **Conversations and messages**: the chat mailboxes of `chat/<slug>.json`.
- **Renderings**: the provenance object beside each compiled PDF, so a
  reader can learn whether a checkpoint has a rendering and what produced
  it without a request to the bucket.
- **The deployment total**: one row, for the storage ceiling.
- **Pending object deletions**: durable cleanup work and the earliest time
  an unreferenced object may be physically removed.

The room that holds a document open stays the in-memory authority and the
broadcast hub for comments and checkpoints. Agent listening heartbeats and
rate-limit counters stay in memory; they are temporary signals, not content.

The bucket keeps bytes: `sessions/<slug>`, the live Yjs state;
`history/<slug>/<sha>`, a checkpoint's tree; `history/<slug>/blobs/<sha>`,
one text apiece; `assets/<slug>/<sha>`; `renderings/<slug>/<sha>` and its
SyncTeX file. It also keeps `rooms/<slug>.lock`, the fenced writer lease,
and `session.key`. Everything in the bucket is either immutable and
content-addressed or, for the session and the lease, a per-document object
with exactly one writer.

The `documents/` and `sources/` prefixes, `legacy_source_key`,
`migrate_legacy_source`, `examples_key`, the rendering provenance objects,
and the case in `history.rs` that reads a checkpoint without a tree as a
tree of one file all go. So do the serde defaults on `IndexEntry`, `Grant`
and `LinkGrant` that describe what a field absent from an older index reads
back as. `IndexEntry` itself is replaced by a `Document` type carrying the
row's fields. Authorisation loads only the caller's membership and presented
link; the sharing dialog loads other people's membership separately. Roles
keep their product meaning, with stable account ids used for ownership,
quota accounting and signed-in authorship.

## One dialect, two places

The database is SQLite in both deployment shapes, so there is one schema
and one set of statements, and the test suite that runs against an
in-memory database exercises the SQL production runs.

- **A directory.** The catalogue is a file, `catalog.db`, beside the
  blobs. This is what running Komodoc on your own machine means, and needs
  nothing installed.
- **A bucket.** The catalogue is a hosted SQLite-dialect database reached
  over the network, so the VPS still holds no durable state of its own.
  Turso is the reference host: its free plan covers this shape at small
  scale, its embedded replicas make reads local and unmetered, and the
  `libsql` crate opens a local file, a remote database and a replica
  through the same connection API. D1 has no equivalent embedded local-file
  replica. Its managed read replicas and HTTP API are a different driver
  shape; compatibility of transactions and migrations would need testing.
  Neither host is declared cheaper without a workload and billing period.

Both go through `libsql`. Embedded replicas forward write transactions to
the primary, including the reads that decide admission. This is the reason
to retain this supported driver here. Turso's newer local-first sync API
does not by itself make admission against shared remote quotas atomic;
changing engines is not required for this work.

`WITHOUT ROWID` is a physical-layout choice, not a billing guarantee. The
schema uses it for the smaller records and starts comments, replies and
messages as ordinary rowid tables. Before freezing the layout, benchmark
both forms with representative average and maximum payloads, measuring
query plans, latency, database size, provider row metrics and sync bytes.
SQLite's large-row guidance is a reason to measure, not proof of a fixed
number of pages per operation. A Postgres port would also change binary
types, connection setup and migrations; it is not just removing a clause.

### Configuration

`--catalog` names the database: a path, or a `libsql://` URL with the token
in `KOMODOC_CATALOG_TOKEN`. Tokens come from the environment for the same
reason bucket credentials do: a flag lands in the process table.

When blobs are a directory and `--catalog` is absent, the catalogue is
`<dir>/catalog.db`. When blobs are a bucket, `--catalog` is required: a
bucket deployment that silently opened a local file would hold its
ownership records on the one disk the design promises nothing lives on,
and losing the VPS would then lose every grant. `serve` prints where the
catalogue is on the same startup line that says where the blobs are.

`--catalog-reads replica|remote` chooses how a remote catalogue is read,
default `replica`; see the read path. `--catalog-sync <duration>` is the
replica's sync interval, default five seconds.

`--catalog-replica <path>` names the local replica, default
`<server-state>/catalog-replica.db`; `--catalog-auth-max-age <duration>`
defaults to fifteen seconds. The latter bounds security-sensitive use of
an unsynchronised replica, not the duration a request may run.

The state directory is mode `0700`; catalogue files, replicas, their WAL
files, and backups are accessible only to the service account (`0600` for
files). Startup prints the database and replica paths without credentials.
Decommissioning removes replicas and sidecar files as well as tokens: a
replica contains the full catalogue, including email and comment text.

The schema is created and later changed by the migrations described below,
which the server applies at startup.

### One active server

This release supports one active server for a deployment. The designated
host holds an exclusive OS lock at `<server-state>/writer.lock` for the
server's lifetime. All local administrative commands use that same lock;
they cannot choose an independent replica path to evade it. Online erase
and other mutations go through the running server. Offline maintenance
requires the server to be stopped and the lock acquired.

The OS lock does not fence a different VPS. Moving hosts requires stopping
and fencing the old process, including revoking its store credentials if
its termination cannot be verified, before giving the new host write
access. There is no automatic lease-expiry takeover. `--single-writer`
asserts this deployment contract and permits omitting the bucket lease;
it does not establish exclusivity by itself. Conditional session writes
remain in both modes.

Multiple active servers are out of scope. Supporting them later requires
primary-enforced writer generations on every SQL mutation, conditional
session writes, coordinated quota reservations, and room invalidation on
revocation and erasure. Merely retaining the bucket lease is insufficient.

### Connection and transaction contract

All user values are bound parameters. Dynamic identifiers and sort choices
come only from fixed allowlists. Connections enable and verify foreign-key
enforcement before use, outside a transaction; the remote driver verifies
the setting on the primary write connection, not only on the replica.
Integration tests demonstrate cascades and rejection of orphan inserts.
Failure to establish these guarantees prevents startup; silently disabling
constraints or replacing only their deletion behavior is not a fallback.

Every operation that reads before deciding a write uses a primary
`BEGIN IMMEDIATE` transaction, including admission, link rotation, guest
pinning and message deduplication. Pure reads use explicit read transactions;
single-statement mutations may use implicit write transactions. No network
or bucket I/O happens while a SQL write transaction is held. Connections
have bounded busy waits and request deadlines. Retry rules below distinguish
an aborted transaction from an unknown commit outcome.

## Schema

```sql
CREATE TABLE accounts (
    id          TEXT PRIMARY KEY,       -- github:<numeric id>, google:<sub>
    provider    TEXT NOT NULL,
    handle      TEXT NOT NULL,          -- login, or verified email
    name        TEXT NOT NULL,          -- what other readers see
    email       TEXT NOT NULL,          -- contact address, '' when none
    first_seen  TEXT NOT NULL,
    last_seen   TEXT NOT NULL,          -- day resolution; see below
    plan        TEXT NOT NULL,          -- '' for free
    status      TEXT NOT NULL CHECK (status IN ('active', 'erasing', 'blocked')),
    session_generation TEXT NOT NULL    -- random; changes on revocation
) WITHOUT ROWID;

CREATE TABLE documents (
    slug            TEXT PRIMARY KEY,
    title           TEXT NOT NULL,
    sha             TEXT NOT NULL,      -- newest checkpoint
    created_at      TEXT NOT NULL,
    published_at    TEXT NOT NULL,      -- last upload of bytes
    updated_at      TEXT NOT NULL,      -- last checkpoint
    example         INTEGER NOT NULL DEFAULT 0,
    owner_key       TEXT NOT NULL,      -- visitor key, or ''; never a login
    owner_id        TEXT REFERENCES accounts (id) ON DELETE RESTRICT,
    status          TEXT NOT NULL CHECK (status IN ('creating', 'active', 'deleting')),
    size            INTEGER NOT NULL CHECK (size >= 0), -- last measured usage
    counted_size    INTEGER NOT NULL CHECK (counted_size >= size), -- reservation
    comment_seq     INTEGER NOT NULL DEFAULT 0,
    source_format   TEXT NOT NULL,
    main            TEXT NOT NULL
);
CREATE INDEX documents_owner ON documents (owner_key) WHERE owner_id IS NULL;
CREATE INDEX documents_owner_id ON documents (owner_id);
CREATE INDEX documents_examples ON documents (slug) WHERE example = 1;

CREATE TABLE grants (
    slug        TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    role        TEXT NOT NULL,          -- 'editor' | 'commenter'
    account_id  TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since       TEXT NOT NULL,
    PRIMARY KEY (slug, role, account_id)
) WITHOUT ROWID;
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
) WITHOUT ROWID;
CREATE UNIQUE INDEX links_hash ON links (hash);

CREATE TABLE guests (
    slug        TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    account_id  TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since       TEXT NOT NULL,
    link_hash   TEXT NOT NULL,
    PRIMARY KEY (slug, account_id)
) WITHOUT ROWID;
CREATE INDEX guests_account ON guests (account_id);

CREATE TABLE totals (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    bytes       INTEGER NOT NULL CHECK (bytes >= 0),
    documents   INTEGER NOT NULL CHECK (documents >= 0)
);

CREATE TABLE checkpoints (
    slug            TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    sha             TEXT NOT NULL,      -- names history/<slug>/<sha>
    seq             INTEGER NOT NULL,   -- position in the timeline
    tree_sha        TEXT NOT NULL,
    parent          TEXT NOT NULL,
    at              TEXT NOT NULL,
    by              TEXT NOT NULL,
    why             TEXT NOT NULL,      -- cli, sync, quiet, comment, ...
    source_format   TEXT NOT NULL,
    size            INTEGER NOT NULL,
    label           TEXT NOT NULL,
    git_commit      TEXT NOT NULL,
    dirty           INTEGER NOT NULL,
    changed         TEXT NOT NULL,      -- JSON array of changed paths
    PRIMARY KEY (slug, sha)
) WITHOUT ROWID;

CREATE TABLE comments (
    slug            TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    id              TEXT NOT NULL,
    seq             INTEGER NOT NULL,   -- the room's ordering, per document
    motivation      TEXT NOT NULL,
    body            TEXT NOT NULL,
    creator         TEXT NOT NULL,      -- the shown name
    author          TEXT NOT NULL,      -- qualified account id, visitor:, link:, or ''
    via             TEXT NOT NULL,      -- link digest it arrived on, or ''
    created         TEXT NOT NULL,
    exact           TEXT NOT NULL,      -- the page selector
    prefix          TEXT NOT NULL,
    suffix          TEXT NOT NULL,
    position        INTEGER,
    region          TEXT,               -- JSON, for a figure comment
    source_path     TEXT,               -- the anchor of record, or NULL
    source_exact    TEXT,
    source_prefix   TEXT,
    source_suffix   TEXT,
    source_position INTEGER,
    proposed        TEXT,               -- a suggestion's replacement text
    outcome         TEXT NOT NULL,      -- '', 'accepted', 'rejected'
    accept_request  TEXT NOT NULL,
    revision        TEXT NOT NULL,      -- checkpoint sha it was made on
    resolved        INTEGER NOT NULL DEFAULT 0,
    resolved_at     TEXT,
    resolved_in     TEXT NOT NULL,
    PRIMARY KEY (slug, id)
);

CREATE TABLE replies (
    slug        TEXT NOT NULL,
    comment_id  TEXT NOT NULL,
    id          TEXT NOT NULL,
    body        TEXT NOT NULL,
    creator     TEXT NOT NULL,
    author      TEXT NOT NULL,
    created     TEXT NOT NULL,
    PRIMARY KEY (slug, comment_id, id),
    FOREIGN KEY (slug, comment_id) REFERENCES comments (slug, id) ON DELETE CASCADE
);

CREATE TABLE conversations (
    slug        TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    id          TEXT NOT NULL,
    token_hash  TEXT NOT NULL,
    expires_at  INTEGER NOT NULL,
    PRIMARY KEY (slug, id)
) WITHOUT ROWID;
CREATE INDEX conversations_expiry ON conversations (expires_at);

CREATE TABLE messages (
    slug            TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    cursor          INTEGER NOT NULL,
    id              TEXT NOT NULL,
    role            TEXT NOT NULL,
    text            TEXT NOT NULL,
    context         TEXT,               -- JSON, or NULL
    PRIMARY KEY (slug, conversation_id, cursor),
    FOREIGN KEY (slug, conversation_id)
        REFERENCES conversations (slug, id) ON DELETE CASCADE
);

CREATE TABLE renderings (
    slug        TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    tree_sha    TEXT NOT NULL,          -- shared tree-content identity, not event sha
    at          TEXT NOT NULL,
    backend     TEXT NOT NULL,
    engine      TEXT NOT NULL,
    release     TEXT NOT NULL,
    tools       TEXT NOT NULL,          -- JSON
    bytes       INTEGER NOT NULL,
    synctex     INTEGER NOT NULL,       -- whether a SyncTeX file sits beside the PDF
    synctex_bytes INTEGER NOT NULL,
    PRIMARY KEY (slug, tree_sha)
) WITHOUT ROWID;

CREATE TABLE pending_deletes (
    slug         TEXT NOT NULL REFERENCES documents (slug) ON DELETE RESTRICT,
    object_key   TEXT NOT NULL,
    bytes        INTEGER NOT NULL CHECK (bytes >= 0),
    queued_at    INTEGER NOT NULL,
    delete_after INTEGER NOT NULL,
    PRIMARY KEY (slug, object_key)
) WITHOUT ROWID;
CREATE INDEX pending_deletes_due ON pending_deletes (delete_after);
```

Thirteen tables. Notes on the ones that are not a plain transcription of a
struct that exists today:

**Accounts.** Sign-in inserts or updates the profile. Activity updates
`last_seen` at most once per UTC calendar day, using a conditional update
that does not rewrite an unchanged row. Profile changes and revocation are
additional writes. Sign-in preserves `plan`, `status` and the current
session generation; it cannot reactivate an erasing or blocked account.
Creation chooses a fresh cryptographically random session generation.
Cookies and device credentials include it and must match an active account
row. A missing row is signed out. Recreating an erased account chooses a
new generation, so old credentials do not become valid again.

`handle` and `name` are read from accounts by join. Documents, grants,
guests and signed-in comment authors use the stable qualified account id,
never a GitHub login. A rename therefore needs no rewrite of their keys.
A visitor's document has `owner_id = NULL` and its visitor key in
`owner_key`; a document with neither has an empty `owner_key` too. For a
signed-in owner, `owner_key` is empty and ownership is decided by
`owner_id` first. An empty key is not permission to bypass a non-null id.
`email` and `plan` are the first fields needed by the retention spec, not
its complete delivery, warning and activity-state implementation.

**Documents.** `published_at` records the last completed publication;
it is not an upload counter. `updated_at` records the newest checkpoint
and orders listings. Account-inactivity retention follows the retention
spec's account activity rules, not this document timestamp. Only `active`
documents appear in normal reads. Creating and deleting rows retain their
ownership and capacity reservation until their work is completed.
`size` is the last reconciled usage; `counted_size` is a conservative
capacity reservation, defined under admission. It replaces the proposed
four-kilobyte undercount, which was not a hard storage ceiling.

**Checkpoints.** The manifest, one row per entry. The tree a checkpoint
names stays in the bucket at `history/<slug>/<sha>`, content-addressed and
immutable, and the text blobs beside it. A document's checkpoints are read
in bounded batches when its room opens, ordered by `seq`. The room enforces
both a configured checkpoint count and a metadata byte ceiling before
allocating the full history; `history_max` defaults to 1,000 for this layout.
The existing shedding policy remains. Pruning rewires the surviving
`parent` fields in the same transaction and preserves the changed-path
summary relative to the new parent, recomputing it before the transaction
if necessary. `changed` retains today's timeline behavior without a tree
GET for every displayed entry. Rewritten parents and summaries cost writes.

**Comments and replies.** The room file, normalised. `Region` is the one
nested value kept as JSON: it is six numbers only ever read and written
together, never queried. The source anchor is columns because a later
feature will want every comment anchored in a path. `author` and `via`
are what the room keeps off every client-bound shape today and go on being
that: they are read to decide `mine` and `deletable` and to group a blind
reviewer's remarks, and never sent. Signed-in `author` is the qualified
account id; display names remain separate snapshots. `documents.comment_seq`
is the durable high-water mark, advanced transactionally when a new comment
needs an ordering number, so deleting the newest comment cannot reuse a
sequence after restart. This adds a document-row update to comment creation.
Comment and reply count caps remain, together with a byte ceiling for their
aggregate stored payload. The room reconstructs these counters from a
fresh snapshot and serialises mutations and erasure through the same gate.

**Conversations and messages.** The mailbox file, normalised. A
conversation's `token_hash` and `expires_at` retain today's meaning. Expiry
is checked on every access, independently of cleanup. Preserve all limits
in `docs/protocol/chat.md`: 16 conversations and 4 MiB per document,
256 messages and 1 MiB per conversation, and the existing message and
request byte limits. Count and byte limits are enforced together in the
primary write transaction. A count limit does not replace a byte limit.

`post` searches the bounded conversation prefix for a repeated message id
before allocating its next cursor. Identical content returns the original
message; different content returns a conflict. Count/byte checks, cursor
allocation and insert are one transaction, so no extra uniqueness index
on `id` is required. Expiry deletes conversations in bounded batches and
cascades to their messages; the cascade is part of the write cost.

The thirty-second listening lease is held only in the active server's
memory, keyed by document and conversation. A restart reports Waiting
until the next authenticated heartbeat. Reads, message retries and
heartbeats do not rewrite durable rows when nothing durable changes.

**Renderings.** One row per `(slug, tree_sha)` whose PDF exists. Checkpoint
event SHAs resolve through `checkpoints.tree_sha`, so restore events can
reuse the same artifact. `bytes` counts the PDF and `synctex_bytes` counts
the optional SyncTeX object. The row advertises each object only after its
upload is durable. The current policy retains the newest rendered source
and labelled checkpoints; an older PDF can be pruned even while its
checkpoint survives. Every such removal deletes or updates its rendering
row and queues the object deletion transactionally, before removing bytes.

**Pending deletions.** One row per object selected for later reclamation.
Its slug's document row survives until cleanup completes, even for an
erased account. These objects still occupy reserved capacity. A durable
queue lets a restart resume cleanup without repeatedly scanning every
document, and protects immutable objects for the recovery window.

**Indexes.** Indexes trade extra writes and pages for fewer scans. Listing
uses the owner and membership indexes, plus a partial examples index whose
entries are written only for examples. Expiration and queued cleanup are
regular background queries and have due-time indexes. Query plans must
demonstrate indexed access; an index that exists but is not used does not
meet this requirement. Rare account erasure may scan author columns, in
bounded passes; add indexes if measured latency or scan cost justifies them.
Ordinary rowid tables also have automatic primary-key indexes. Billable
rows and sync bytes are measured separately from logical affected rows.

## Migrations

Nothing is released, so there is nothing to migrate from today. There will
be, and a schema with no migration story is one that gets changed by hand
on a live database. The policy is fixed now so the first release ships
with it.

**Numbered, embedded, forward only.** A migration is a file under
`crates/komodoc/migrations/`, named `NNNN_what_it_does.sql`, compiled into
the binary and applied in order. The schema above is `0001_catalog.sql`.
A fresh database is an empty one with every migration applied, so there is
no separate schema file to keep in step with the migrations, and a
database created on the day of release and one created a year later are
the same rows arrived at the same way. There are no down migrations.
Undoing one is restoring the backup taken before it, which is the only
undo that is ever actually tested.

**Versioned by `PRAGMA user_version`.** The database records the number
of the last migration applied. The binary knows the highest number it
ships. At startup the server compares them:

- Database behind: apply each pending migration, in order, each in its
  own transaction. SQLite's DDL is transactional, so a migration that
  fails leaves the version where it was and the server refuses to start,
  with the migration's name and the error.
- Database level: start.
- Database ahead: refuse to start. An older binary must never run against
  a shape it does not know. The message names both versions.

For each pending migration, begin a primary `BEGIN IMMEDIATE` transaction,
re-read `user_version`, and apply that migration plus its version update
atomically. Commit before beginning the next migration; there is no outer
transaction containing nested `BEGIN`s. The deployment writer lock remains
held across backup and the entire sequence. Migration tests also exercise
competing driver connections, though concurrent application servers are
not a supported deployment mode.

**Code when SQL is not enough.** Some changes need the program, such as
re-sealing every link key under a new derivation. Such a migration is a
Rust function registered in the same numbered sequence, run in the same
transaction discipline, and named the same way. The sequence is one list
whichever kind each entry is.

**Before applying.** While the deployment is quiescent, create one verified
recovery point for the pending sequence and record the original version.
For a directory catalogue use SQLite's online backup API to produce
`catalog.db.before-NNNN`; a raw copy may omit committed WAL data. Check the
backup's integrity and version before applying anything. For a hosted
catalogue record a provider-supported restore point and verify that it is
available under the configured plan. Capture mutable sessions and the
sealing secret as described under recovery. Failure to create the recovery
point prevents migration. `komodoc migrate --catalog <where>` holds the
same exclusive lock; `--dry-run` lists migrations without writing.

**Deploying a schema change.** Stop the old server and drain its writes,
take the recovery point, then migrate with the new binary under the lock.
Synchronise or rebuild the replica and check its version before serving
requests. No older server continues running during this sequence.

**Tests.** Every migration ships with a test that builds a database at the
previous version with representative rows, applies the migration, and
checks the rows come through. A further test applies all migrations from
empty and asserts the version matches the highest the binary ships, so a
file added without being registered fails before it is released.

**Compatibility rule.** A migration may add, rename or drop anything,
since the binary that ships it is the only one that will run against the
result. What a migration may not do is depend on data the previous version
did not guarantee: a column the old binary allowed to be empty is migrated
with an explicit rule for empty, written in the migration, not left to a
default.

## Link keys at rest

Everything else in these tables has to be readable to do its job. The link
key does not: it is read only when an owner asks to copy a link again, and
in every other request the caller presents it and the server matches the
hash. So it is the one column stored encrypted, and a copy of the
catalogue on its own then holds no usable bearer credential. An attacker
needs the bucket as well, where `session.key` lives.

The sealing key is derived from `session.key` with HKDF-SHA256 under a
versioned fixed label. Use XChaCha20-Poly1305 with a fresh random 24-byte
nonce stored with the ciphertext; the longer nonce does not remove nonce
generation or storage. Associated data binds the encoding version, slug,
role and link hash using an unambiguous length-delimited encoding. Verify
the recovered key hashes to the stored hash. A database-only read exposes
neither the raw link key nor the sealing secret; encryption does not
protect against an attacker who can rewrite authorization records.

`session.key` is minted only when both stores describe an empty deployment.
A nonempty catalogue with a missing or unreadable key refuses startup.
Backups include the key with the same access restrictions as the stores.
An administrative rotation holds the writer lock, verifies recovery of the
old key, reseals links and changes the root secret through a recoverable
maintenance operation; it is never an unnoticed startup side effect.

Replacing the secret alone invalidates signed credentials but does not
revoke links checked by hash. Emergency rotation after compromise must
explicitly revoke those links and their guest pins as well. An unreadable
sealed value is an error to report to the owner, not proof that its bearer
credential expired. Startup must not reset hashes or discard ciphertext
in an attempt to repair it.

## The trait

The server, rooms, janitor and seed command see a `Catalog` trait, never a
connection. The old in-memory `Store` is removed; content helpers operate
on `BlobStore`. Operations describe product actions and update only the
rows they need. There is no closure that reads and rewrites a whole entry.

The caller is authenticated before entry, and writing operations recheck
account status, session generation, document status and required rights
inside the primary transaction. An in-process per-document mutation gate
covers the authorization decision and the operation; account erasure also
uses the account gate. Gates have one documented acquisition order.
Checks made before awaiting a request body are repeated afterward.

Accounts:

- `sign_in(Profile)`, `touch_activity(id, day)`, `account(id)`.
- `revoke_sessions(id)`: change the generation and disconnect that account.
- `begin_erasure(id)`: mark erasing and revoke its generation atomically.

Documents and access:

- `get(slug)`: document metadata, without other people's memberships.
- `authorize(slug, caller)`: the active document, active account and
  generation, the caller's grant, and the presented link needed for the
  existing role decision. It never loads all guests or returns sealed keys.
- `sharing(slug, cursor, limit)`: an owner-only, paginated membership
  view. Copying a link is a separate owner-authorized operation.
- `prepare_publication(slug, Publication, capacity)`: admit an upload
  before object writes. A new document gets a hidden creating row;
  replacement reserves peak capacity while leaving the current head live.
- `commit_publication(slug, Publication)`: publish the durable head and
  its first checkpoint together; activate a creating row. Failed preparation
  writes no object. Failed completion retains its reservation for recovery.
- `rename(slug, title)`, `adopt(slug, owner)`.
- `grant`, `revoke`, `set_link` and `drop_link`: atomically change
  membership; rotating or dropping a link also removes its guest pins.
- `pin_guest(slug, account_id, link_hash)`: skip the write when the local
  row already matches this link. Otherwise validate the account and current
  link at the primary and insert or conditionally update the row. A stale
  replica cannot re-pin a revoked link. An unchanged row is a no-op.
- `visible(caller, cursor, limit)`: indexed owner, grant, guest and
  optional-example branches, combined without duplicate documents, with
  keyset pagination by `updated_at, slug`. Guest visibility validates the
  current link and its expiry. Tests inspect the complete query plan.
- `reserve_capacity(slug, required)`, `reconcile_usage(slug, usage)`
  and `room_for(slug)`: see admission.
- `begin_delete(slug)`, `finish_delete(slug)`: the durable lifecycle
  below; ordinary readers cannot retrieve deleting rows.

Checkpoints:

- `checkpoints(slug, cursor, limit)`: bounded, ordered history reads.
- `record_checkpoint(slug, Checkpoint, usage, format, main)`: insert the
  event and update the document head in one transaction. Reconcile usage
  and capacity if needed. A repeated event id is a no-op only if its
  recorded content agrees.
- `label(slug, sha, label)`.
- `prune(slug, shas, surviving_updates, garbage)`: delete events,
  rewrite surviving parent/path summaries, update rendering availability,
  and queue newly unreferenced objects in one transaction. Compute
  reachability before this transaction under the document lifecycle gate.

Comments:

- `comments(slug, cursor, limit)` and bounded reply reads reconstruct a
  room from one consistent snapshot, subject to the byte and count caps.
- `add_comment`, `add_reply`, `resolve`, `decide`,
  `delete_comment`, `delete_reply`: staged room mutations, committed
  before broadcast. Creation ids are stable across retries; conflicting
  reuse fails. Creating a comment advances the durable sequence. Deleting
  a comment cascades to replies and reports their ids for room invalidation.

Conversations:

- `conversation`, `open_conversation`, `post`, `messages`,
  `delete_conversation`, `expire_conversations(now, limit)`.
  Reads authenticate the independent chat token and check expiry.
  Presence operations authenticate too, but do not write the catalogue.

Renderings:

- `rendering(slug, tree_sha)`, `record_rendering`, and
  `retire_renderings(slug, trees)`. Publishing a PDF and later publishing
  its SyncTeX object are distinct durable transitions. Retirement updates
  availability and queues bytes even when their checkpoint still exists.

Erasure and housekeeping:

- `footprint(account_id)`: owned documents, memberships and authored
  comment/reply counts, with paginated details.
- `forget_batch(account_id, cursor, limit)`: remove authored comments,
  replies and memberships in bounded transactions while the account is
  erasing, returning affected ids. `finish_erasure(id)` checks that no
  ownership, authored rows, pins or grants remain before removing it.
- `expired(before, from, cursor, limit)`: candidates only; deletion
  rechecks the governing retention policy and current activity.
- `queue_deletes`, `deletes_due(now, cursor, limit)` and
  `finish_object_delete`: resumable reclamation.
- `all(cursor, limit)`: bounded inventory for offline seeding and slow
  orphan audits, never a request-path enumeration of the deployment.

Each durable operation commits atomically. Multi-step product operations
have durable lifecycle states rather than pretending to share a transaction
with the bucket. The driver returns a known success, a known abort, or an
unknown outcome. A lost commit response is not evidence of rollback.

After an unknown outcome, block later mutations for that document and
reconcile against the primary using the operation's stable ids and expected
postconditions. Do not replay counter increments or broadcast a second
event blindly. Reads of an affected room wait or return a retryable error
until memory matches committed state. If the primary remains unavailable,
keep the operation unresolved; a restart reloads authoritative state before
serving that room. A confirmed abort can undo the staged in-memory change.
An acknowledged save or comment always means its durable write succeeded.

`StoreState`, index versions, whole-index reloads and index CAS disappear.
Manifest and mailbox CAS disappear too. `BlobStore::swap` remains for
session snapshots and, when enabled, the room lease. Session persistence
keeps its write gate, ETag conflict handling, generation check and rejection
of stale writers. Tree/blob request coalescing remains.

## Admission and storage accounting

A storage ceiling bounds stored product payload, including the live session,
checkpoint trees and distinct text blobs, assets, renderings, comments and
messages. Objects retained for cleanup or recovery still count until they
are actually removed. Shared infrastructure, SQLite page/index overhead,
account records and backup snapshots are deployment overhead with separate
capacity monitoring and a budget; payload quotas do not predict a provider's
physical storage bill.

For signed-in owners every sum uses `owner_id`. Visitor and unattended
documents use `owner_key` with `owner_id IS NULL`. A handle change cannot
split a quota, and adoption checks the receiving account's capacity.
Creating and deleting documents count toward byte and document ceilings.

The accounting invariants are:

```text
actual retained payload for a document <= documents.counted_size
documents.size = last reconciled payload measurement
totals.bytes = SUM(documents.counted_size)
totals.documents = COUNT(documents)   # all lifecycle states
```

`counted_size` is reserved capacity, not an approximation allowed to
undercount. `StorageLimit.usage_reserve_bytes` defaults to 64 KiB.
`reserve_capacity` grows a document's reservation in chunks, using a
smaller exact increment near a ceiling if necessary. In one primary
`BEGIN IMMEDIATE` transaction it checks the owner's reserved-byte sum,
the deployment total and any document-count change, then changes the
document and total together. Its aggregate reads run at the primary and
are metered even when ordinary reads use an embedded replica.

Before accepting an edit or starting an upload, the active server ensures
that its conservative upper bound on resulting retained payload fits the
document's reservation. This includes concurrent uploads, simultaneous old
and new objects during replacement, SQL content growth, and bytes awaiting
physical deletion. Room gates serialise local allocation within a reserved
chunk. Capacity needed by a different document cannot spend this chunk.
When additional capacity cannot be reserved, refuse that new mutation
before accepting it; previously accepted edits still persist.

A room reconstructs usage and outstanding cleanup from authoritative
metadata and bucket inventory once on load. Failed or uncertain writes
retain their capacity until reconciled; they never release it on assumption.
Checks compare both count and byte ceilings before allocating large row
collections or request bodies. Comments and replies share a payload ceiling
of `max_annotations` (default 256 KiB); mailbox limits remain independent.

An ordinary session persist writes no catalogue row while its encoded state
fits the reservation. A checkpoint reconciles `size`; asset/rendering
completion and SQL content mutations maintain the active server's usage
counters and may reconcile in an existing transaction. Reconciliation also
runs before relinquishing a room or completing cleanup. It may retain a
bounded spare chunk for future edits. A restart reconciles before releasing
capacity. There is no assumption that a checkpoint occurs within a fixed
time: the reservation protects quotas even when checkpoints are deferred.

The total changes only when the reservation changes. Removal subtracts
`counted_size`, not the last measurement in `size`. Replacement applies
the change from the old reservation; adoption changes ownership without
changing deployment bytes or document count. Unused reserved capacity is
shown separately from measured usage. Deletion may free space only after
the advertised recovery/cleanup window; the UI states this before deletion.

`room_for` is informational headroom. It is not an authorization to write:
only an atomic reservation grants additional capacity. Two concurrent
reservations competing for the last megabyte cannot both succeed.

`uploads_per_hour` counts actual admitted create/replace attempts in a
sliding per-owner process window. Replacing one slug repeatedly consumes
one token per new attempt; retrying the same unresolved operation does not
admit another upload. There is also a deployment-wide admission rate.
Rejected authorization and quota requests reach no object write. These
counters survive room eviction but reset with the server; they are abuse
controls during an uptime interval, not a persistent monthly billing cap.
The rates are recorded in the workload model, and tests exercise repeated
replacement of one document rather than counting recent document rows.

## Two stores, one order

There is no transaction across the bucket and SQL. Public references are
committed only after their objects are durable; removal first withdraws
the public reference, then schedules physical reclamation.

All object materialisation, retirement and deletion for a slug share a
lifecycle gate. The gate covers bucket I/O and the final SQL transition,
but no SQL transaction spans that I/O. Operations protect every object
they may publish, including an old content-addressed object being reused.
On restart, recover creating/deleting documents and uncertain operations
before serving them or running cleanup. The one-writer deployment contract
also applies to the janitor and all administrative commands.

- **Create.** Reserve capacity and create a hidden row; write the session,
  tree and blobs; then atomically activate the document with its checkpoint.
  A crash leaves recoverable preparation, not a publicly reachable broken
  document. Cleanup cannot race the preparation gate.
- **Replace.** Reserve temporary peak usage and stage immutable content,
  preserving the current public head. Commit the new head only after its
  objects exist. Serialise any live-session replacement with room writes
  and use its ETag; retain the prior session in protected recovery storage
  until completion. A crash between the session write and head commit
  requires reconciliation before the room reopens, not an ordinary read of
  a half-replaced document.
- **Delete.** Under the gate, mark the document deleting and invalidate
  access, drain existing writers and close sockets. Reject new opens.
  Withdraw associated public metadata and queue all document objects.
  Cleanup retries after crashes. Only after bucket deletion is confirmed
  and all pending jobs are gone may `finish_delete` remove the row,
  cascade remaining children and subtract its reservation exactly once.
  A deleting slug cannot be republished until this lifecycle finishes.
- **Checkpoint.** Protect the input snapshot, reserve any growth, write new
  blobs and the tree, then commit the event and head. Queue superseded
  material only after computing references from surviving checkpoints,
  live session content and other operations in progress.
- **Prune.** Retire rows and update surviving metadata together with
  deletion jobs. Queue a blob only when no retained tree or active
  operation needs it. Rendering retirement follows the PDF retention
  policy independently of whether the checkpoint survives.
- **Comment.** Stage the memory change and required checkpoint, commit
  durable rows, then publish the event. A confirmed abort undoes staging;
  an unknown commit outcome invokes reconciliation. A pruned revision is
  resolved for display using the existing oldest-surviving fallback; it is
  not silently rewritten into a different historical claim.
- **Rendering.** Write the PDF before creating availability metadata.
  A later SyncTeX upload changes availability only after its own write.
  Retirement removes availability before queuing either object.

### Bounded reclamation

Normal cleanup consumes `pending_deletes`, ordered by due time. Each pass
has explicit row, object-request, byte and elapsed-time budgets (defaults:
100 queue entries, 1,000 bucket requests, 64 MiB read, and thirty seconds).
At a budget boundary it yields; SQL transactions contain small batches,
not the whole queue. Repeated storage failures use bounded exponential
backoff. Successful deletion followed by a lost SQL response is safe to
retry: an already missing object counts as deleted, while quota release
is transactional and occurs only once.

Before physical deletion, reacquire the document lifecycle gate, consult
the primary, and recheck all live references and protections. Reusing an
object cancels its queued deletion before it becomes publicly reachable.
If any reference scan is incomplete or a retained tree is unreadable,
delete nothing from that candidate set. A replica miss is never evidence
that bytes are unreferenced.

`BlobInfo` gains an object modification timestamp, populated from S3
listings or filesystem metadata. A newly discovered orphan must pass both
the object-age grace period (default one hour) and a recovery delay starting
when it was first confirmed unreferenced. Object age alone is insufficient:
old objects can be reused, and writes can outlast a grace period.

A separate, slow orphan audit traverses paginated inventory under the same
request and byte budgets; it does not walk every retained tree hourly.
Its scan cursor may restart after process loss, but already queued jobs are
durable. Unknown slug prefixes are reported and reconciled into hidden
cleanup records with measured usage before reclamation; absence of a row
is not an instruction to delete a prefix. Existing active, creating or
deleting rows and their gates protect concurrent work. Counters expose the
audit's progress, requests and bytes so its operating cost is visible.

### Recovery window

`--recovery-window` defaults to 24 hours and must not exceed the verified
catalogue restore window. A queue entry's `delete_after` is no earlier
than its retirement time plus this window. Trees and blobs remain
recoverable even after public rows retire. Increasing the window does not
retroactively restore already deleted objects. Retained bytes remain
charged, and the configured window is part of the cost model.

A recovery point for a migration includes the catalogue version and backup,
`session.key`, and a snapshot of mutable session objects while writers
are stopped. Recovery snapshots live in a private, bounded
`recovery/<backup-id>/` namespace or the operator's protected backup
destination; document routes never serve it. Their expiry and storage
budget are explicit. The queue protects immutable objects needed by every
supported restore point. Verify a complete recovery point before resuming
writes or destructive cleanup.

Restoring SQL alone is not a recovery procedure. Stop writers and cleanup,
restore the matching secret and mutable-session snapshot when available,
restore or retain all required immutable objects, rebuild the replica, and
verify references and usage before serving. Catalogue point-in-time restore
without a matching session snapshot recovers checkpointed content at that
point; it does not promise point-in-time recovery of every acknowledged Yjs
update. Normal VPS replacement still recovers the current durable session
objects. State these distinct recovery guarantees to the operator.

Tests restore after checkpoint pruning, rendering retirement and document
deletion, and after a crash with committed WAL data. The future retention
policy's suggested thirty-day recovery period is not promised by this
24-hour configuration; that feature must provision matching recovery
capacity and catalogue retention.

## The read path

A directory catalogue uses indexed local reads. A hosted catalogue selects
between embedded replica reads and remote reads; neither changes where
admission and conditional writes are decided.

- **`replica`, the default.** Pure reads run locally. The primary handles
  write transactions and the replica must observe its own successful
  writes before the operation returns. Initial download, catch-up and
  periodic page replication contribute to synchronization usage. The
  replica is disposable, but contains private data and is protected as
  described in configuration.
- **`remote`.** Reads execute at the primary through bounded connections
  and batched queries. They incur network latency and metered row scans.
  This can be cheaper when replication traffic is high relative to reads;
  it can be more expensive for a read-heavy deployment. Select the mode
  using measured scanned rows, sync bytes and latency, not a fixed number
  of checkpoints per day.

A startup replica is not usable for protected requests until a complete
primary sync and schema check succeeds. Record the monotonic completion
time of each successful sync. While its age is at most
`catalog_auth_max_age`, protected reads and session-only updates may use
it. Beyond that age, reject new protected operations with a retryable
service-unavailable response and suspend or close their sockets. Do not
treat a stale authenticated caller as anonymous, or clear their cookie
because storage is unavailable. Public content whose access requires no
identity can remain available.

The freshness limit bounds use of stale permissions, not network recovery
time. Expiry timestamps are also checked against current time on every
request and socket authorization check. Account/session revocation, link
rotation, grant changes and deletion invalidate affected live sockets as
part of the local operation. Every incoming mutation frame checks the
current room authorization state; idle sockets are checked at least once
per second so revoked or stale connections stop receiving protected data.

Session updates accepted before a freshness failure may still be flushed
within their already reserved capacity. No additional edits or capacity
are accepted on the assumption that the primary would approve them.
Account erasure and destructive cleanup always consult the primary.
The single-server contract prevents a second application process from
silently changing permissions behind a room's in-memory state.

`authorize` returns only the caller's decision inputs. Public responses
never expose contact email, private author ids, token hashes, sealed link
keys or internal lifecycle state. Sharing and footprint queries are
separate, authorized and paginated. Tests assert bounded query results for
a document with many guests and no full-deployment scan for example listing.

## The write budget

Separate service limits from workload estimates. A bill depends on editing
time and timing, retained history, file changes, readers and guest arrivals,
database payload sizes, retries, cleanup, backups and replica rebuilds.
Daily editor count alone does not determine it.

### Persistence and admission rates

`SessionLimit.persist_floor_seconds` defaults to fifteen.
`persist_max_dirty_seconds` also defaults to fifteen and must be at least
the floor. Two seconds of quiet remains an opportunity to save earlier
when the floor permits it. Otherwise the oldest unsaved update reaches the
maximum dirty age and schedules a snapshot even if updates never stop.
The maximum-age deadline does not reset on every keystroke.

The floor is measured between snapshot write starts, not acknowledgements;
healthy storage latency is additional to the scheduling delay. Maintain
the deadline for edits arriving during an in-flight snapshot. Only edits
included in a successful durable write receive a saved acknowledgement.
Failures preserve dirty state and retry with backoff; no fixed durability
latency is promised during a storage outage.

Disconnect and eviction do not discard dirty state or bypass the floor.
Keep the room until its pending snapshot is durable. Graceful shutdown
drains scheduled snapshots; a forced shutdown can lose unacknowledged
edits, which must never have been reported saved. Retain recent per-slug
write timestamps across room eviction for at least the floor interval.

Turn `rooms_max` into a hard admission limit, including rooms still
loading. Evict clean idle rooms first; if no slot is available, refuse the
new room with a retryable capacity response. Existing active rooms continue.
Today's idle-eviction target is not a hard cap and cannot be used to prove
a worst-case bill. Bound aggregate room payload memory as well, and account
for decoded allocation overhead when selecting that limit.

Uploads, asset/rendering writes, comments, mailbox mutations, sharing
changes and room creation also pass authenticated per-caller and
deployment-wide rate limits before expensive work. Use the existing
rate-limiter mechanism, with explicit configuration and metrics, rather
than one SQL row per token. Limits count attempts that reach costly work,
not only successful results, and repeated message ids are no-op retries.
A deployment cannot claim a monthly spend cap from these process counters:
they reset on restart, and already accepted saves must still be drained.

### Checkpoint policy

Ordinary sync checkpoint requests coalesce over two minutes; ordinary CLI
requests retain the thirty-second window. Every newly created checkpoint,
whatever its reason, consumes the per-owner checkpoint budget (default
300 per rolling hour). Counters outlive rooms and reset only with the
server. A request that reuses an existing exact checkpoint costs no new
checkpoint token. Client-supplied reasons or request ids cannot bypass
the budget.

Quiet and last-editor-leaving checkpoints may be deferred. Saving their
live session does not depend on creating that mark. Preserve one pending
request per document; on restart a live source differing from the head
can request a fresh mark. A comment, restore, suggestion acceptance or
rendering action that requires an exact new checkpoint must secure its
budget and capacity before accepting that action. If it cannot, return a
retryable rate-limit response without applying the action or associating
it with an older source. A previous edit remains durable.

This makes the tradeoff explicit: the system can defer timeline marks and
refuse an additional action; it cannot promise both unlimited
action-triggered checkpoints and a bounded checkpoint write stream.

### Operations to measure

An ordinary checkpoint inserts an event and updates the document head.
This is two logical rows before reservation changes, pruning, parent/path
rewrites, rendering retirement and queue maintenance. Changed text files
still require separate blob PUTs; keeping trees out of SQL does not make
bucket cost independent of the number of changed files.

A comment creation also advances the document sequence. A reply,
resolution or decision usually changes one logical row, but deleting a
comment can cascade to many replies. Conversation expiry similarly
deletes all its messages. Queueing and completing an object deletion,
automatic primary-key indexes, secondary indexes, aborted writes and
retries all belong in metering. Direct statement affected-row counts do
not measure these costs.

Session persists inside a capacity reservation cause no SQL writes.
Reservation increases and usage reconciliation do. Account activity is
at most one day-stamp update per active account per UTC day, plus real
profile/security changes. Existing guest pins, presence heartbeats and
idempotent retries do not issue a needless durable update.

Measure page replication directly. A small row update can send a complete
database page; two logical rows do not guarantee two pages, or a fixed
number of frames. Include indexes, overflow, tree splits, initial sync,
catch-up, every replica and backup traffic. Embedded replicas use page-level
replication; Turso recommends measuring newer logical sync separately
rather than assuming equal bandwidth. [Embedded replica documentation](https://docs.turso.tech/features/embedded-replicas/introduction)

### Pricing and workload worksheet

Reference prices checked 2026-09-07, USD, for the plans shown. Billing
frequency and allowances are separate inputs, not constants in the program.

| Turso plan | Monthly billing | Annual billing, monthly equivalent | Writes/month | Reads/month | Sync/month |
| --- | ---: | ---: | ---: | ---: | ---: |
| Free | $0 | $0 | 10 million | 500 million | 3 GB |
| Developer | $5.99 | $4.99 | 25 million | 2.5 billion | 10 GB |
| Scaler | $29 | $24.92 | 100 million | 100 billion | 24 GB |

Developer overages are $1/million writes, $1/billion reads and $0.35/GB
of sync, with storage billed separately. These are reference inputs to
refresh before deployment. [Turso pricing](https://turso.tech/pricing?frequency=monthly),
[annual billing](https://turso.tech/pricing?frequency=yearly)

D1 Free allows 100,000 writes/day, while its paid allowance is 50 million
writes/month. Its free allowance alone does not establish the cheaper paid
deployment, especially when comparing remote reads with embedded sync.
[D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/)

For R2 Standard, PUT and LIST are Class A operations at $4.50/million;
GET and HEAD are Class B at $0.36/million. Storage, included allowances
and billing-unit rounding must also be applied. Other bucket providers
have different request and transfer charges. [R2 pricing](https://developers.cloudflare.com/r2/pricing/)

This illustrative table assumes 200 continuously active, resident rooms,
30 days, and regular snapshot writes at the floor. It excludes other
operations, cold-room churn, retries and free allowances; it is not a
deployment-wide worst-case bill.

| Floor | Regular snapshot PUTs/month | R2 Class A cost before allowances |
| --- | ---: | ---: |
| 15 seconds | 34.56 million | about $156 |
| 60 seconds | 8.64 million | about $39 |
| 120 seconds | 4.32 million | about $19 |

The formula is `rooms * active_seconds / floor_seconds`. Increasing the
floor requires increasing the allowed dirty age; the table does not choose
that durability tradeoff for the operator. Neither an 80% saving nor a
25–33% saving follows from average writes per hour: measure pause timing.

The deployment worksheet records, for each scenario:

- Active room-seconds, new room arrivals, pause traces and forced shutdowns.
- Checkpoints by reason, changed files, payload sizes and retained history.
- Comments/replies, mailbox activity, distinct guests and ordinary reads.
- Logical writes, provider-reported writes and scans, and measured sync GB.
- Bucket PUT/GET/HEAD/LIST/DELETE requests, bytes stored and any transfer.
- Reclamation, reservation slack, recovery copies, backup operations,
  failed attempts and cold replica rebuilds.

Report both a normal workload and a sustained-load case, with observed
latency and refusal rates. Compare remote and replica modes using those
same scenarios. The previous cents-per-thousand-checkpoints and
thousand-editor monthly totals are withdrawn until this worksheet is
measured. Storage or row-budget exhaustion must stop new admission
predictably; it must not trigger garbage collection against stale data.

## Erasing an account

Self-service and operator erasure use one resumable operation keyed by the
qualified account id, not a supplied login-based author key.

1. **Freeze and revoke.** Under the account mutation gate, set status to
   erasing and replace the session generation in a primary transaction.
   Reject new publications, adoption, comments, grants and sign-ins for
   that identity. Disconnect all of its live sockets and drain operations
   already admitted before enumerating ownership. Profile/contact fields
   can be cleared now; keep the stable id and erasing state needed to finish.
2. **Withdraw owned documents.** Mark each owned document deleting through
   the normal lifecycle. Hide it and close its rooms immediately. Physical
   object reclamation remains queued for the recovery window. Re-enumerate
   ownership before completion; an earlier footprint is not authoritative.
3. **Remove other authored data and membership.** In bounded transactions,
   remove the account's comments, replies, grants and guest rows on remaining
   documents. A deleted parent comment takes its replies, whoever authored
   them. Clear explicit checkpoint author attribution to the erased id
   while retaining other people's document content and history. Each pass
   advances a primary-key scan cursor, including over nonmatching rows,
   so a rare author does not cause a repeated full scan per deletion.
   Invalidate affected rooms under their gates, remove replies as well as
   comments, and reauthorize membership-dependent sockets before releasing
   the gate or resuming broadcasts.
4. **Finish.** After owned documents and pending cleanup are gone, verify
   that no owned rows, authored records or memberships remain and delete
   the account row. The document ownership foreign key prevents premature
   removal. Re-running a completed erasure is a no-op. A fresh sign-in
   after completion creates an account with a new session generation.

Erasure status reports logical removal separately from physical cleanup,
including the expected recovery-window deadline and retryable cleanup
failures. New sign-in cannot cancel an erasure still in progress. Clearing
the initiating browser's cookie is a UI action; invalidating the account
generation is what revokes every device. A blocked account is a retained
status, not an account deletion that the next OAuth login can undo.

The confirmation explains what remains. Edits in other people's text and
checkpoint content belong to those documents and are retained. Anonymous
and automation-link remarks cannot be attributed to the account merely
because the same person may have written them. Provider backups and private
recovery snapshots expire on their stated schedules. Do not describe logical
deletion as immediate physical erasure or promise recovery beyond the
configured and tested window.

**Self-service.** The account menu shows `footprint`, including owned
titles and counts, and asks for the current handle to confirm. The server
derives the target id from the authenticated account, checks its current
generation against the primary, and applies the existing cross-site request
protections. A visitor or link-only identity cannot erase an account.
The operation returns a resumable status instead of keeping a request
open throughout a potentially day-long physical cleanup.

**Operator.** `komodoc erase --account <qualified-id-or-handle>` resolves
a unique stable account id, prints the footprint and asks for confirmation
as the current command design requires. When the server is running it uses
the service-account-only local administrative socket at
`<server-state>/admin.sock` (mode `0600`), so the server's gates and
invalidation run. Offline it acquires the deployment writer lock and runs
the same lifecycle. It reports logical removal, queued cleanup and final
completion separately; it never writes around a live server's room state.

## Seeding and tests

`seed` is an offline destructive reset under the same deployment writer
lock. It clears bucket data and catalogue rows in a foreign-key-safe order,
then publishes examples through the normal publication lifecycle and inserts
their annotations with an empty author. Partial failure leaves the server
offline and the reset resumable. It is not a cross-store transaction.

Unit tests execute the actual embedded migration files against an in-memory
database and a temporary blob directory. Add file-backed SQLite tests for
WAL, connection setup, concurrent transactions and restore: in-memory tests
cannot establish those guarantees.

The same catalogue contract suite runs through the remote driver and an
embedded replica when `KOMODOC_TEST_CATALOG` names an isolated test database.
That suite may be skipped in routine local runs; the release job requires
it and fails if its database or credentials are unavailable.

Required behavior and fault-injection cases include:

- Foreign-key enforcement on every local/primary connection; cascades;
  ownership and pending-deletion references preventing premature erasure.
- Atomic competing capacity reservations; stable-id quotas after rename;
  exact totals after remove/adopt/replace; growth without checkpoints;
  failed uploads retaining capacity and reconciliation after restart.
- Repeated uploads of one slug consuming rate tokens; alternate checkpoint
  triggers, reconnects and request ids not bypassing budgets.
- A continuously changing session reaching its dirty-age deadline;
  floor enforcement through eviction/reconnect; acknowledgements covering
  only the persisted generation; hard room admission including cold loads.
- Duplicate message ids with equal and conflicting content; byte limits
  before count limits; concurrent cursor allocation; no-op guest pins and
  memory-only presence producing no catalogue writes.
- Revocation on every device, account recreation with a new generation,
  erasure racing publication/comments, removal after handle changes,
  replies under other authors' comments, and invalidation of open rooms.
- Replica startup before first sync, extended sync failure, expired links,
  read-your-writes, and refusal of protected operations after maximum age.
- Lost responses after committed writes, known aborts, stalled bucket
  operations, and restart before room memory has been reconciled.
- Cleanup racing new uploads and reuse of old objects, incomplete tree
  scans, pending preparations, repeated deletion, and stale replica misses.
- Rendering retirement while a checkpoint survives, shared rendering
  identity after restore, and independent PDF/SyncTeX availability.
- Recovery from committed WAL data and restoration after pruning/deletion,
  with catalogue, secret, immutable objects and mutable snapshots verified.
- Indexed, paginated authorization/listing/expiry queries and bounded
  erasure/cleanup work on large, mostly unrelated datasets.

Metering tests distinguish logical affected rows from cascades, index work
and provider-reported writes/scans. Measure replication bytes and bucket
operations around representative workflows, including cleanup and cold
starts. An added index or changed payload layout must report the measured
cost difference; a simple assertion that an UPDATE affected one row cannot
serve as that check. Benchmarks compare physical table layouts before
treating either as cheaper.

## Deploy

`make deploy` provisions the hosted database and scoped token, the private
server-state directory, the common writer-lock path, and recovery storage.
Runtime credentials are limited to this catalogue and bucket; provisioning
credentials are not kept by the serving process. The operator configures
the persistence, admission, cleanup and recovery limits and checks the
workload worksheet against the selected provider allowances.

A release stops and drains the old server before obtaining the lock for
backup and migration. It verifies the recovery point, applies migrations,
refreshes the replica and performs startup integrity checks before accepting
traffic. A failed rollout restores the complete recovery point under the
lock; it does not launch the old binary against a newer schema. The same
rule applies to planned key resealing: restore both catalogue and secret
on failure before any serving process resumes.

Metrics cover reserved versus measured payload, pending deletion age/bytes,
provider capacity, row scans/writes, sync traffic, bucket operations, replica
freshness, queued saves, retries and admission refusals. Alert before plan
allowances are exhausted. A provider rejecting writes must leave prepared
operations, acknowledged data and deletion reservations recoverable.

## Related specs

`docs/specs/history.md` lists remaining history features, including
pinning and rendering fallback; it does not specify the current storage
layout. This spec defines checkpoint rows, trees and blobs while preserving
the existing restore, comparison and timeline behavior.

`docs/specs/retention.md` defines account activity, contact delivery,
warnings and the eventual recovery policy. The account fields here are
a starting point, not an implementation of that entire policy. Cleanup
must not substitute a document timestamp for owner activity.

`docs/specs/sync.md` and `docs/protocol/chat.md` describe synchronization
and the conversation protocol. SQL storage preserves independent chat
tokens, expiry, byte/count caps, idempotent messages and presence behavior.

## Out of scope

Multiple active application servers, automatic failover, billing,
organisations, search and a Postgres driver are not implemented here.
Cross-document comment feeds and checkpoint search remain future product
features. Their eventual SQL queries and indexes need a workload and cost
assessment; moving records into SQL does not make them free.
