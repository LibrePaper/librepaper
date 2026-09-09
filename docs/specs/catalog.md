# SPEC: transactional catalogue

## Decision

LibrePaper has one storage design with two deployment profiles:

- **Local:** SQLite is authoritative and immutable objects live in the same
  private deployment directory. This is the simple self-hosted profile.
- **Hosted:** Turso is authoritative and immutable objects live in R2. This is
  the durable, scalable profile for production services.

This replaces the whole-file JSON indexes and per-room snapshot objects.
[persistence.md](persistence.md) defines the edit journal and “saved.”
[failover.md](failover.md) defines recovery and future takeover.

Both profiles use the same schema, batching coordinator, segment format,
publication state machine, recovery rules and contract tests. Product code
depends on `Catalog` and `ObjectStore` interfaces; it does not branch on the
profile. Only the drivers and the durability statement differ.

The first release includes migrations, indexed reads, atomic admission, stable
document identities, guarded batch publication, bounded replay, compaction and
journal retirement and resumable account erasure. It uses one active server and
a zero document recovery window. Positive document recovery windows may follow;
durable cleanup and complete backup recovery are required from the first release.

## Data placement

The catalogue holds the small mutable or queryable records in the schema below.
The object store holds checkpoint trees and blobs, assets, renderings, recovery
bases and shared journal segments. Presence, listening leases and rate counters
stay in memory.

Local deployments use `<deployment>/catalog.db` and
`<deployment>/objects/`. Hosted deployments use Turso and R2; their optional
embedded catalogue replica is private and disposable.

Every document receives a random immutable `storage_id`. Document-owned objects
use `content/<storage_id>/trees/<sha>`, `blobs/<sha>`, `assets/<sha>` and
`renderings/<tree_sha>/` below that same identity prefix. Routes resolve an active
slug to its current identity; clients never supply an arbitrary object key.
Journal bases and shared segments use the separate journal namespace in
[persistence.md](persistence.md).

A deleting row reserves its slug until its objects and journal payload have been
reclaimed. Only then may a new document reuse the slug, with a new `storage_id`.
SQL children may therefore remain keyed by slug. Immediate slug reuse during
cleanup is not supported. Shared journal objects have journal-owned retirement
metadata and are never deleted by a per-document prefix sweep.

This layout removes `index.json`, room and mailbox JSON, history JSON manifests,
`sessions/<slug>`, bucket writer leases and `--single-writer`. Session signing
keys also leave the bucket; see Secrets and recovery. Legacy source/document
prefix readers and compatibility defaults are removed. The initial migration
creates a fresh catalogue; automatic import of the old layout is outside this
work. Detect an existing legacy deployment and refuse startup with an explicit
export/republication instruction. Never silently treat it as an empty store.

## Configuration and connections

`serve <directory>` selects the local profile. It creates `catalog.db`,
`objects/`, `state/` and `secrets/` below that directory. Its server-state path
is always `<directory>/state`.

`serve --catalog <libsql-url> --bucket <r2-bucket>` selects the hosted profile
and requires `LIBREPAPER_CATALOG_TOKEN` plus R2 credentials. Hosted mode also
requires an absolute `--server-state <path>` for its private disposable state.
Reject partial or mixed configurations. Credentials and secret-file locations
come from the environment and are never printed with their contents.

Hosted `--catalog-reads replica|primary` defaults to `replica` for the pure reads
permitted under Authorization and reads.
`--catalog-sync` defaults to five seconds. The replica defaults to
`<server-state>/catalog-replica.db`. The state directory is mode `0700`; files
are mode `0600`. Decommissioning removes the replica and its sidecars because
they contain private catalogue data.

Each deployment permits one active server. Hold an exclusive OS lock at
`<server-state>/writer.lock` for the process lifetime. Administrative commands
use the same configured path; choosing another replica path cannot bypass it.
Online mutations go through the running server, including operator erasure.
Offline maintenance requires the writer to stop and the same lock to be held.
Replacing a hosted server additionally requires fencing the old host before
the replacement receives Turso or R2 access. A different state directory does
not establish a second writer's authority.
Hosted journal publication checks a durable writer generation at the Turso
primary. Automatic takeover remains disabled until
[failover.md](failover.md) passes its gates.

All user values use bound parameters. Dynamic identifiers and sort choices use
fixed allowlists. Enable and verify foreign keys outside a transaction on every
authoritative connection, including new pooled connections, reconnects and
maintenance connections. Failure prevents that connection from serving work.
Every read-before-write decision, including quota admission, runs inside an
authoritative `BEGIN IMMEDIATE` transaction. Multi-statement pure reads use one
read transaction. No object-store request occurs inside a SQL transaction.
Busy waits, requests and result sets are bounded.

Hosted drivers use libSQL primary write transactions and embedded replicas with
read-your-writes enabled, or explicit primary reads. Local-first offline writes
are not an admission mechanism. Local SQLite uses WAL and `synchronous=FULL`;
the driver verifies its durability settings before accepting writes. File-backed
tests cover committed WAL recovery as well as the in-memory SQL contract.

## Schema

The catalogue starts with eleven domain tables and a document-operation
ledger. Journal records follow the durable record contract in
[persistence.md](persistence.md) and use the same migration sequence.

```sql
CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    handle TEXT NOT NULL,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    plan TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'erasing', 'blocked')),
    session_generation TEXT NOT NULL,
    erasure_cursor TEXT CHECK (length(CAST(erasure_cursor AS BLOB)) <= 65536)
) WITHOUT ROWID;

CREATE TABLE documents (
    slug TEXT NOT NULL PRIMARY KEY,
    storage_id TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    sha TEXT NOT NULL,
    created_at TEXT NOT NULL,
    published_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    example INTEGER NOT NULL DEFAULT 0,
    owner_key TEXT NOT NULL,
    owner_id TEXT REFERENCES accounts (id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('creating', 'active', 'deleting')),
    size INTEGER NOT NULL CHECK (size >= 0),
    counted_size INTEGER NOT NULL CHECK (counted_size >= size),
    maintenance_reserved INTEGER NOT NULL DEFAULT 0
        CHECK (maintenance_reserved >= 0 AND maintenance_reserved <= counted_size),
    comment_seq INTEGER NOT NULL DEFAULT 0,
    last_auto_checkpoint_at INTEGER NOT NULL,
    pending_publication TEXT,
    last_publication_id TEXT NOT NULL DEFAULT '',
    source_format TEXT NOT NULL,
    main TEXT NOT NULL
);
CREATE INDEX documents_owner ON documents (owner_key) WHERE owner_id IS NULL;
CREATE INDEX documents_owner_id ON documents (owner_id);
CREATE INDEX documents_examples ON documents (slug) WHERE example = 1;
CREATE INDEX documents_expiry_created ON documents (created_at, slug)
    WHERE status = 'active' AND example = 0;
CREATE INDEX documents_expiry_updated ON documents (updated_at, slug)
    WHERE status = 'active' AND example = 0;

CREATE TABLE grants (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    role TEXT NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since TEXT NOT NULL,
    PRIMARY KEY (slug, role, account_id)
) WITHOUT ROWID;
CREATE INDEX grants_account ON grants (account_id);

CREATE TABLE links (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    role TEXT NOT NULL,
    hash TEXT NOT NULL,
    sealed BLOB NOT NULL,
    label TEXT NOT NULL,
    budget INTEGER,
    since TEXT NOT NULL,
    until TEXT NOT NULL,
    PRIMARY KEY (slug, role)
) WITHOUT ROWID;
CREATE UNIQUE INDEX links_hash ON links (hash);

CREATE TABLE guests (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since TEXT NOT NULL,
    link_hash TEXT NOT NULL,
    PRIMARY KEY (slug, account_id, link_hash)
) WITHOUT ROWID;
CREATE INDEX guests_account ON guests (account_id);
CREATE INDEX guests_link ON guests (slug, link_hash);

CREATE TABLE totals (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    documents INTEGER NOT NULL CHECK (documents >= 0)
);

CREATE TABLE checkpoints (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    sha TEXT NOT NULL,
    seq INTEGER NOT NULL,
    durable_seq INTEGER NOT NULL CHECK (durable_seq >= 0),
    tree_sha TEXT NOT NULL,
    parent TEXT NOT NULL,
    at TEXT NOT NULL,
    by TEXT NOT NULL,
    why TEXT NOT NULL,
    source_format TEXT NOT NULL,
    size INTEGER NOT NULL,
    label TEXT NOT NULL,
    git_commit TEXT NOT NULL,
    dirty INTEGER NOT NULL,
    changed TEXT,
    PRIMARY KEY (slug, sha)
) WITHOUT ROWID;
CREATE UNIQUE INDEX checkpoints_sequence ON checkpoints (slug, seq);

CREATE TABLE comments (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    motivation TEXT NOT NULL,
    body TEXT NOT NULL,
    creator TEXT NOT NULL,
    author TEXT NOT NULL,
    via TEXT NOT NULL,
    created TEXT NOT NULL,
    exact TEXT NOT NULL,
    prefix TEXT NOT NULL,
    suffix TEXT NOT NULL,
    position INTEGER,
    region TEXT,
    source_path TEXT,
    source_exact TEXT,
    source_prefix TEXT,
    source_suffix TEXT,
    source_position INTEGER,
    proposed TEXT,
    outcome TEXT NOT NULL,
    accept_request TEXT NOT NULL,
    revision TEXT NOT NULL,
    resolved INTEGER NOT NULL DEFAULT 0,
    resolved_at TEXT,
    resolved_in TEXT NOT NULL,
    PRIMARY KEY (slug, id)
);
CREATE UNIQUE INDEX comments_sequence ON comments (slug, seq);

CREATE TABLE replies (
    slug TEXT NOT NULL,
    comment_id TEXT NOT NULL,
    id TEXT NOT NULL,
    body TEXT NOT NULL,
    creator TEXT NOT NULL,
    author TEXT NOT NULL,
    created TEXT NOT NULL,
    PRIMARY KEY (slug, comment_id, id),
    FOREIGN KEY (slug, comment_id)
        REFERENCES comments (slug, id) ON DELETE CASCADE
);

CREATE TABLE renderings (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    tree_sha TEXT NOT NULL,
    at TEXT NOT NULL,
    backend TEXT NOT NULL,
    engine TEXT NOT NULL,
    release TEXT NOT NULL,
    tools TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    synctex INTEGER NOT NULL,
    synctex_bytes INTEGER NOT NULL,
    PRIMARY KEY (slug, tree_sha)
) WITHOUT ROWID;

CREATE TABLE pending_deletes (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE RESTRICT,
    object_key TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    queued_at INTEGER NOT NULL,
    delete_after INTEGER NOT NULL,
    PRIMARY KEY (slug, object_key)
) WITHOUT ROWID;
CREATE INDEX pending_deletes_due ON pending_deletes (delete_after);

CREATE TABLE catalog_operations (
    storage_id TEXT NOT NULL REFERENCES documents (storage_id) ON DELETE CASCADE,
    request_id TEXT NOT NULL CHECK (length(CAST(request_id AS BLOB)) BETWEEN 1 AND 128),
    kind TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('prepared', 'committed', 'aborted')),
    intent TEXT NOT NULL CHECK (length(CAST(intent AS BLOB)) <= 65536),
    result TEXT NOT NULL CHECK (length(CAST(result AS BLOB)) <= 65536),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (storage_id, request_id)
);
CREATE INDEX catalog_operations_pending ON catalog_operations (storage_id, request_id)
    WHERE status = 'prepared';
```

Use `WITHOUT ROWID` only for narrow tables. Comments and replies remain rowid
tables because their bodies can be large. Verify indexes with
query plans and measure their write and replication cost.

## Record rules

- Account ids are stable provider ids, never mutable logins. Signed-in authors
  use the account id; display names are separate. Cookies carry a random
  `session_generation` and are valid only while the matching active account row
  exists. Revocation changes the generation for every device. Sign-in updates
  profile fields but preserves plan, status and generation; it refuses an
  erasing or blocked account. A new account gets a fresh generation only when
  no row exists. Update `last_seen` conditionally, at most once per active UTC
  day. `erasure_cursor` is bounded, versioned stage/table/primary-key progress
  metadata, NULL outside erasure; committing a batch also advances its cursor.
  Qualifying activity is authenticated owner use under
  [retention.md](retention.md), not visits by other readers. Contact delivery
  and inactivity-warning workflows remain work for that spec.
- Signed-in ownership uses `owner_id` and an empty `owner_key`. Visitor ownership
  uses `owner_id = NULL` and a verified visitor key. An empty owner key never
  bypasses a non-null account id.
- Only `active` documents appear in normal reads. `creating` and `deleting`
  rows retain ownership and reserved capacity while work is unfinished.
  Documents owned by an erasing account are withdrawn immediately, even before
  the erasure worker marks each document deleting.
  `pending_publication` names the prepared operation for this `storage_id`.
  Set and clear it in the same transaction as the operation's state change.
- `size` is reconciled usage. `counted_size` is its conservative reservation.
  Ordinary session persists within that reservation do not update the document
  or total rows.
- Checkpoint trees and text remain immutable objects. `changed = NULL` means
  unknown after parent rewiring; an empty list means known unchanged. Compute a
  requested comparison on demand instead of reading trees during pruning.
  `seq` orders timeline events; `durable_seq` identifies the journal sequence
  represented by the checkpoint. Never prune the current head. Read history
  and comments using sequence cursors and bounded pages, not offsets or a full
  manifest allocation.
- `documents.comment_seq` prevents comment sequence reuse. Signed-in comment
  and reply authors, `via` and internal request markers remain hidden from
  client payloads. Preserve `max_comments = 500` per document and
  `max_replies = 100` per comment, existing request/body limits, and
  `max_annotations = 256 KiB` for serialized seed annotations. SQL storage
  does not remove these limits; room loading also enforces its memory budget.
- Human and agent chat are ephemeral as defined by
  [the chat protocol](../protocol/chat.md). Neither belongs in this schema,
  storage accounting, backup, recovery, or account erasure.
- A rendering row is visible only after its PDF exists and is keyed by
  `(slug, tree_sha)`. A later SyncTeX upload sets availability only after its
  own object is durable. Retirement withdraws availability and queues the
  objects atomically, even when the corresponding checkpoint survives.
- `pending_deletes` covers document-owned objects. Shared edit segments use
  journal retirement metadata from [persistence.md](persistence.md).

Times stored as text use canonical UTC RFC 3339 with fixed precision so indexed
comparisons agree with time ordering. `links.until = ''` means no expiry; any
other value must be a valid timestamp. Invalid stored expiry fails closed.

Opening a link checks the eligible read source for the exact
`(slug, account_id, link_hash)` pin. An existing pin needs no write. Otherwise,
the insert transaction rechecks the active account and generation, active
document, and matching unexpired link at the primary. Rotation or revocation
removes matching pins in the same transaction. Listings join pins to current
live links, so expiry hides them even before cleanup runs. A pin records a visit;
it never substitutes for presenting the link when accessing the document.

## Catalogue operations and outcomes

The server, rooms, janitor and administrative commands use `Catalog`, rather
than issuing SQL or editing whole records themselves. Its operation families
have these postconditions; a family may have several methods:

| Operations | Contract |
| --- | --- |
| Account/profile, activity, session revocation | Preserve account lifecycle state; revocation invalidates every device. |
| Authorize, visible documents, sharing | Load only the decision inputs or requested page; never expose secrets or other users' private identifiers. |
| Prepare/commit publication, reserve/reconcile, transfer | Update intent, ownership, reservation and totals atomically as applicable. |
| Pin guest, rotate/drop link, change grant | Revalidate at the primary and invalidate affected pins, cached permissions and sockets. |
| Checkpoint, label, prune, rendering publication/retirement | Commit heads, availability and deletion work consistently with durable objects. |
| Comment/reply creation, resolution, acceptance, deletion | Commit before acknowledgement/broadcast; return affected ids for room reconciliation. |
| Begin/finish deletion, erasure batches, expiry candidates, cleanup | Use durable lifecycle state and bounded cursors; release capacity only after reclamation. |

User-initiated mutations recheck account status, generation, document lifecycle
and rights in their primary transaction, including after awaiting an upload
body. Product operations acquire account gates in account-id order, then
document lifecycle gates in `storage_id` order, then the journal publication
gate when needed. Never acquire an earlier gate while holding a later one.
The coordinator must not hold its queue mutex while waiting for these gates.

Callers enqueue compound source/metadata operations without retaining room or
document gates. The coordinator acquires those gates once, includes or flushes
earlier admitted updates in sequence order, and owns staging through publication.
It does not recursively acquire a gate already held by the calling operation.

`catalog_operations` records recoverable document mutations; request ids are
nonempty and at most 128 UTF-8 bytes. The versioned
`intent` JSON contains the expected document head and durable sequence, actor
and generation, operation kind, proposed metadata, required reservation and
protected object keys/digests/lengths. It contains no document bodies. Reject
an intent exceeding 64 KiB before writing objects. `result` holds the compact
original outcome, including allocated ids, sequences and head. The canonical
request digest binds all supplied content and options. A retry with different
content is a conflict. The ledger is private and is charged to the deployment's
catalogue budget; reserve enough metadata capacity to complete admitted work.

For SQL-only mutations, insert a committed receipt in the same transaction as
the change. For object-backed mutations, reserve capacity and insert a prepared
operation before object writes; completion changes it to committed with the
new references. A confirmed abort retains cleanup responsibility for any
objects already written. Retain document-operation receipts until the document
is physically removed, including tombstones for deleted comments; receipt
growth is part of admission and the measured storage cost. Journal batches use
the separate bounded reconciliation records in [persistence.md](persistence.md).

Every driver returns known success, known abort or unknown outcome. On an
unknown outcome, query the primary by operation id before allocating another
comment sequence, repeating side effects, releasing capacity or
reporting failure. `last_publication_id` is only a fast path; the ledger proves
earlier outcomes after the head advances. Suspend dependent work and serving
of uncertain room state until reconciled; unrelated documents may continue.
Retrying an already committed operation returns its recorded outcome after
current authorization checks. Account lifecycle changes are reconciled against
their expected/new generation and status and cannot be blindly repeated.

Stage room changes under the lifecycle gate, commit them, then update cached
state and broadcast. A known abort discards staging. An unknown outcome reloads
the affected state from the primary before broadcasting or accepting dependent
operations. A comment that needs an exact revision obtains its checkpoint
budget and durable revision before its row becomes visible.

## Secrets and recovery

Session signing and link sealing use separate random 256-bit keys. Local mode
stores them as `<deployment>/secrets/session.key` and `links.key` in a `0700`
directory with `0600` files. Create them durably only for a verified empty
deployment. Hosted mode requires `LIBREPAPER_SESSION_KEY_FILE` and
`LIBREPAPER_LINK_SEALING_KEY_FILE`, provisioned from protected external storage or
a secret manager; the disposable state directory is not their recovery source.
A nonempty catalogue with a missing or unreadable key refuses startup.

Link keys are random, stored by digest for lookup and sealed at rest with
XChaCha20-Poly1305. Store a versioned envelope containing a key id, random nonce
and authenticated ciphertext. Associated data binds the format, `storage_id`,
role and digest using an unambiguous encoding; verify the decrypted key's digest.
Neither signing nor sealing secrets belong in the catalogue or R2. Copying a
link requires owner authorization, separate from ordinary metadata reads.

Key rotation is an explicit maintenance operation under the writer lock. Keep
the old and new sealing keys recoverable while resealing rows in resumable
batches keyed by envelope key id. Verify all rows before removing the old key
from runtime use; supported backups still need their matching key versions.
Rotating the signing key invalidates signed credentials. Resealing alone does
not revoke bearer links; compromise recovery explicitly rotates/revokes those
links and their pins. Schema migration never silently changes either key.

Both profiles recover a matched catalogue, immutable object graph, deployment
identity and both secret versions. Initial backup support uses independently
retained object copies. Pause new admissions, drain/reconcile admitted and
prepared work, and pause cleanup under the deployment lock. Then take the SQLite
backup or named Turso restore point and copy every reachable object, including
journal manifests, bases and tails. Never advertise a backup of unresolved state.
Verify digests and availability of the matching secrets, then durably publish
a completion manifest before resuming writers or cleanup. The manifest records
the schema/head, restore-point id, object inventory, secret version identifiers
and expiry; it never contains the secrets themselves. Local backups keep secret
files in a separately protected part of the backup. Hosted backups rely on
externally retained secret versions.

Backup objects use a private `recovery/<backup-id>/` namespace or a separate
backup destination. Normal reclamation never visits that namespace. Partial
backups are not advertised as recovery points and have their own resumable
cleanup; completed backups expire as units under an explicit storage budget.
Only named points with verified complete copies are supported. Turso's other
point-in-time restore positions are not automatically application recovery
points. Thus a zero document recovery window does not shorten backup retention.

## Migrations

Migrations are embedded, numbered, forward-only files under
`crates/librepaper/migrations/`. Each runs in its own primary transaction and
updates `PRAGMA user_version`. Startup applies missing versions, accepts an
exact match and rejects a database newer than the binary.

Before migration, stop and fence the writer and verify a complete recovery
point under Secrets and recovery. Restore preparation and cleanup must finish
before affected documents are served. Rebuild a hosted disposable replica
afterwards. Tests create the previous schema with representative rows, apply
each migration and also build the latest schema from empty. The hosted contract
explicitly verifies transactional DDL, `PRAGMA user_version`, rollback on a
mid-migration failure and reopening through the replica. There are no down
migrations; rollback restores the complete verified recovery point.

## Authorization and reads

Local reads use SQLite. Hosted pure reads may use the embedded replica;
admission and every mutation use the Turso primary. A successful authoritative
write must be visible to its caller immediately. Authorization changes have a
stronger contract: every affected local authorization path must observe the
committed change before it can serve another protected operation. Hold the
affected gates, invalidate caches and suspend sockets, and advance the shared
replica through that commit before releasing them. Primary mode reads the
committed primary state directly. If synchronization or the commit outcome is
uncertain, keep the affected access suspended until primary reconciliation and
catch-up succeed. Clearing a memory cache cannot permit repopulating it with
an older authorization row.

Before the first protected replica read after startup, complete a primary sync
and verify the schema. Measure successful sync age with a monotonic clock.
`--catalog-auth-max-age` defaults to 60 seconds: this permits a brief sync outage
while keeping a one-minute upper bound on using an unsynchronized replica. It
does not delay known local revocations. When stale, protected HTTP requests
return a retryable `503` and protected sockets suspend. Public content requiring
no identity or link may continue to be served. Expiry is always checked against
current server time; an unavailable account lookup never downgrades a signed-in
caller to anonymous or clears their credential.

Every authenticated request verifies that the account row exists, is active
and has the cookie's generation. A brief memory cache is allowed only with
immediate local invalidation. In hosted mode it never extends validity past the
replica-age bound.

Check current room authorization on each incoming mutation and before protected
broadcasts. Revalidate idle sockets at least once per second for expiry and
replica freshness. Already admitted edits may finish persistence within their
reservations after access is revoked; new edits are refused. Deletion and
erasure drain these admitted edits through their lifecycle gates.

Visible-document queries combine indexed owner, grant, live-guest and optional
example branches, deduplicate by slug and use `(updated_at, slug)` cursors.
Sharing membership and reply reads are separately authorized and paginated.
Query-plan tests bound rows examined on large unrelated datasets; a small
`LIMIT` alone is not evidence of bounded work. Public payloads never include
email, account author ids, token digests, sealed keys or lifecycle intents.

## Admission and accounting

All quota decisions happen at the authoritative catalogue inside one
`BEGIN IMMEDIATE` transaction. Per-owner sums are indexed primary-side
aggregates over document rows, not separately stored counters. Ownership and
document-count limits include every lifecycle state. The invariants are:

```text
actual retained document payload <= documents.counted_size
documents.size = last reconciled retained-payload measurement
totals.bytes = SUM(documents.counted_size)
totals.documents = COUNT(documents)                  # every lifecycle state
user reservation = counted_size - maintenance_reserved
SUM(user reservation) <= storage.total
SUM(user reservation for owner) <= storage.per_owner
SUM(maintenance_reserved) <= maintenance_reserve_bytes
```

Retained payload includes text, assets, renderings, annotation content,
and attributable journal bases/frames, including prepared and retired copies
until reclaimed. Shared framing, SQL/index/receipt overhead and independent
backups have separate measured budgets. Provider or local physical capacity
must cover all of these, not just `storage.total`.

Creation reserves its known requirement plus at most
`usage_reserve_bytes = 64 KiB` of spare capacity. Incremental growth uses that
same maximum spare bound and a smaller exact increment near a ceiling. A known
large upload reserves its entire peak requirement in one transaction. Do not
round whole-document requirements to powers of two. Capped adaptive increments
may be introduced only with measurements and without increasing the spare
bound. A background pass may shrink slack on idle, fully reconciled documents;
never shrink while a write, publication or deletion is unresolved. Display
measured bytes, ordinary reservations, maintenance borrowing and spare capacity
separately. Reconcile after restart before releasing any capacity.

`maintenance_reserve_bytes` defaults to 64 MiB of additional capacity unavailable
to ordinary uploads or edits. A bounded rewrite may borrow it by atomically
increasing the affected documents' `counted_size`, `maintenance_reserved` and
`totals.bytes`. The durable job records each allocation. The borrowing covers
temporary copies without requiring another user's free quota. Release it only
as physical reclamation and accounting transitions complete; a crash retains
the allocation. Serialize maintenance jobs when their combined worst-case peak
would exceed the reserve, and split rewrites into bounded steps. Admission must
ensure each accepted document/segment can be rewritten within the configured
reserve. Keep separate SQL and filesystem working headroom for completion and
cleanup. Test progress with ordinary user capacity fully allocated.

Preserve per-owner upload-attempt limits and add a configurable deployment limit.
New admitted replacements consume a token even when the slug already exists;
retrying one recorded operation does not. Rate counters outlive rooms and reset
only with the process. Quota admission happens before object writes.

## Publication and reclamation

All object materialization, reference changes and physical deletion for a
document share its lifecycle gate. Hold it across object I/O and the final SQL
transition, while keeping SQL transactions short. Protect old content-addressed
objects being reused as well as new uploads. Recovery resolves prepared
operations before opening affected rooms or allowing their cleanup.

Creating and replacing content follows this order:

1. Reserve peak capacity and record a durable intent; creation inserts a hidden
   row and replacement preserves the currently published head.
2. Write immutable objects outside the SQL transaction.
3. Publish the durable source reference, exact checkpoint, document metadata,
   measured usage and committed operation receipt in one catalogue transaction.
4. Reconcile unknown commit outcomes by operation id before retrying.

Replacement applies a server-authored update to the existing Yjs document under
the room's mutation gate. Preserve its CRDT identities and use the existing
publish/merge semantics, including concurrent and returning offline clients.
Do not replace it with a fresh CRDT or install a recovery base as a content-edit
shortcut. Journal the staged update through the coordinator; the transaction
publishing its segment also publishes the new checkpoint/head and receipt.
Discard uncommitted staging on a known abort; reconcile before exposing it after
an unknown outcome. Initial creation publishes a complete recovery base because
there are no pre-existing clients or identities to preserve.

Suggestion acceptance follows the same rule: commit the journal reference,
checkpoint, `outcome`, `accept_request` and `resolved_in` together. Budget or
capacity refusal precedes applying the suggestion. Broadcast and acknowledge
only the committed edit and metadata. The coordinator must support these
compound publications without holding SQL transactions across object I/O.

Removing a document marks it deleting, withdraws access, drains admitted writers
and closes its room before reclaiming objects. Retire shared payload through the
journal. Keep the row and its reservation until every document-owned deletion
and journal charge is resolved; then delete it and subtract the remaining
`counted_size`, not `size`, exactly once. A missing object is a successful
idempotent deletion. Changing ownership moves its ordinary reservation between
owner aggregates but does not change the deployment total; recheck the new
owner's capacity and active status in the transfer transaction.

Object reclamation uses durable queues. Each pass is bounded by 100 jobs,
1,000 object requests, 64 MiB read and 30 seconds, whichever comes first; SQL
transactions process small batches and storage failures back off. Orphan audit
has a separate persisted inventory cursor and the same per-pass budgets,
running at most hourly. It is a safety net, not the primary deletion mechanism.

Before deletion, acquire the relevant lifecycle gate and recheck primary live
references, active readers, prepared operations and retention protections.
Reusing an object cancels queued deletion before publication. Incomplete
reference scans or unreadable retained trees forbid deleting the candidate.
`orphan_grace_seconds` defaults to 3600: an unknown object must be both older
than this and continuously recorded as unreferenced for this long. Object-store
listings expose modification time; a missing timestamp prevents orphan deletion.
A long-running preparation remains protected regardless of its age. Track
orphans under unknown identities in journal/maintenance metadata, with measured
charges, before reclaiming them; a replica miss never authorizes deletion.

`recovery_window` is the document undelete window and is fixed at zero for the
initial release. A future positive value retains deleted objects and their
capacity until expiry; it cannot bypass lifecycle or reader protections.
Independent backups follow Secrets and recovery, with separate retention and
charges. Normal cleanup never removes their copies.

Preserve `--expire` and `--expire-from created|updated` for deployments that
configure document TTL. Select non-example active candidates with a timestamp
and slug cursor using the corresponding partial index. Under the lifecycle gate,
recheck the timestamp against the current primary before beginning normal
deletion. Account-inactivity cleanup instead follows owner activity under
[retention.md](retention.md); document timestamps cannot substitute for it.

## Persistence, history and cost controls

The batching schedule and acknowledgement rules live in
[persistence.md](persistence.md). `rooms_max` is a hard admission limit,
including rooms still loading, and `rooms_bytes_max` defaults to 512 MiB.
Evict clean idle rooms first; refuse new rooms retryably when no slot remains.
Never evict dirty work. The byte cap covers conservatively accounted resident
CRDT state, loaded annotations/history and room-owned queues, including loading
and staging allocations. Encoded Yjs length alone is not heap usage. The shared
coordinator and process overhead have separate bounded budgets and are included
in measured peak RSS. Enforce growth limits on existing rooms as well as opens.

Automatic history changes from five minutes of quiet to an hourly schedule.
`history_interval_seconds` defaults to 3600. Initialize
`last_auto_checkpoint_at` at initial publication and advance it only when an
automatic checkpoint commits. A background scheduler checks at least once per minute;
when the interval has elapsed and durable source differs from the latest
checkpoint, it queues an automatic mark. Continuous edits do not reset the
deadline. With available storage, budget and service capacity, an eligible mark
commits on the next scheduler/flush cycle.

Pending history work survives room eviction: derive it from committed journal
coverage and checkpoint `durable_seq` on restart, and include unopened documents
in scheduling. A document edited for ten minutes and abandoned still gets its
mark when due. Read and checkpoint the covered durable sequence under its gate;
later edits remain candidates. An unchanged source creates no duplicate mark.
Deferred history does not change whether its journaled edits are saved.

Exact-version actions may create additional checkpoints. All new checkpoints
consume configurable rolling-hour budgets, initially 300 per owner and 10,000
per deployment. Automatic marks additionally obey the per-document interval;
an existing exact checkpoint consumes no new token. Counters outlive rooms.
Clients cannot evade budgets through supplied reasons or new request ids.
Budget exhaustion defers automatic work and refuses an exact-version action
before its side effects. The default `history_max = 0` imposes no checkpoint
count cap; storage, metadata-memory and rate limits still apply.

The request model and release workload are defined once in
[persistence.md](persistence.md#cost-and-release-checks). Measure provider-reported
operations and replication bytes rather than inferring them from logical rows.
Include admission aggregates, receipts, activity updates, checkpoints by reason,
assets, renderings, cleanup and complete backup copies in that workload.

## Erasure

Account erasure is resumable and keyed by stable account id:

1. Under the account gate, mark the account `erasing`, change its session
   generation, reject new work and disconnect its sockets. Drain already
   admitted operations before enumerating ownership. Sign-in and transfer
   cannot reactivate or add ownership to an erasing account.
2. Mark its documents deleting and run normal reclamation.
3. In bounded primary-key passes, remove its grants, guest rows, comments and
   replies from other documents and clear explicit checkpoint attribution.
   A deleted parent comment also removes its replies. Advance a durable cursor
   across matching and nonmatching rows. Each batch takes the affected document
   gates and updates or invalidates their cached room state before broadcasting;
   disconnecting only the erased account's sockets is insufficient. Remove
   attribution from operation receipts as well; retain only the anonymous
   request identity/result needed to prevent replay of a completed action.
4. Re-enumerate ownership and authored data. After owned rows, authored records
   and cleanup are gone, delete the account row. Its foreign keys prevent
   premature deletion. Repeating completed cleanup is harmless.

Logical withdrawal and physical cleanup are reported separately. Retained
content belonging to other documents, recovery windows and provider backups are
explained to the user. Sign-in is refused while erasure is pending. Only after
completion may it create a fresh account row and generation; old credentials
cannot revive. A blocked account remains a retained blocked row. Self-service
erasure derives the target from the authenticated account and requires current
primary authorization and the existing cross-site request protections.

## Verification and deployment

The release job runs the same catalogue and persistence contract against a
temporary file-backed local deployment and an isolated Turso database with an
R2 bucket. Hosted tests exercise primary reads and embedded replicas; missing
credentials fail the release job rather than silently skipping it. Required
cases include:

- Foreign keys on newly created/reconnected connections, cascades, transactional
  DDL/version updates, migration rollback, WAL recovery and complete restore.
- Competing reservations, exact totals in every lifecycle state, bounded spare
  capacity, maintenance borrowing and full-user-quota deletion/compaction.
- Known aborts and unknown commits for publications and comments,
  acceptance and account lifecycle changes; equal/conflicting retries and
  receipts surviving later mutations.
- Live replacement and suggestion acceptance interrupted between journal and
  metadata preparation/publication, including connected and offline clients.
- Immediate revocation on every device, primary-to-replica read-your-writes,
  refusal before initial sync, prolonged sync failure and idle socket expiry.
- Concurrent rotation and guest insertion, multiple live pins for one account,
  expiry-filtered listings and bounded sharing/history/comment queries.
- Byte limits before count limits, annotation caps and hourly history for
  continuous editing, abandoned rooms, eviction, restart and budget exhaustion.
- Cleanup racing object reuse, long preparations, unreadable trees, unknown
  orphans, bounded expiry/deletion, shared-segment retirement and slug reuse.
- Rendering retirement while its checkpoint survives, separate PDF/SyncTeX
  availability, interrupted erasure and invalidation of other users' room caches.
- Local directory-entry durability, missing secrets, resumable resealing,
  restored sealed links, zero-window deletion after backup, and fresh-host
  recovery without the original state directory.

Offline `seed` is an explicit destructive reset under the deployment writer
lock, after a verified backup. Clear data in foreign-key-safe order and publish
examples through the normal lifecycle with new storage identities and the
existing deterministic example slugs. Keep the server offline until a partial
reset is reconciled; retain backups and secrets. Remote seeding uses the running
server's authorized delete/publication paths and observes pending deletion.

Hosted deployment provisions scoped runtime credentials. Both profiles create
a private state directory and writer lock and require a tested backup. Stop and
drain the old server for migrations. Monitor reserved and measured bytes,
maintenance borrowing, receipt/catalogue overhead, queued deletion, replica
freshness, save/history queues, admission refusals, provider operations and sync
traffic. Alert before configured allowances or maintenance headroom are exhausted.

Simultaneous application writers, billing, organisations and search are out of
scope. Automatic failover is a planned stage governed by
[failover.md](failover.md).
