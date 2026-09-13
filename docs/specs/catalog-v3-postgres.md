# Catalog v3: PostgreSQL and simple immutable storage

Status: implemented replacement architecture; provider-specific deployment
acceptance remains an operator release check. This specification starts from the
product's requirements rather than preserving catalog v2's schema, storage
protocols, or invariants. LibrePaper is unreleased software; implementation may
make breaking changes and need not convert a catalog v2 deployment.

## 1. Objective

LibrePaper shall use a conventional relational catalog and immutable blob
storage. The design must be inexpensive for one operator on one machine and
must permit the hosted service to move the database, blobs, application
processes, and workers onto separate machines without changing the product data
model.

The baseline architecture is:

```text
browser
   |
reverse proxy
   |
LibrePaper application processes
   |                     \
PostgreSQL                filesystem or S3-compatible blob storage
   |
LibrePaper worker processes
```

PostgreSQL is the only catalog implementation. An operator may run PostgreSQL
on the same machine as LibrePaper over a Unix socket. A hosted deployment may
use managed PostgreSQL. These are deployment choices, not separate application
storage modes.

The design favors retaining inexpensive immutable bytes over maintaining exact
physical-object reachability. It favors coarse quotas and provider spending
limits over billing-grade byte reservations. It favors ordinary database rows
and endpoint-specific idempotency over a general durable operations engine.

## 2. Product requirements

The storage architecture supports:

- registered, anonymous, and example document ownership;
- Markdown, HTML, Typst, LaTeX, and Quarto projects;
- projects with multiple text files, images, bibliographies, data, and other
  input assets;
- live collaborative source editing;
- comments, highlights, replies, and source suggestions;
- named access grants and revocable share links;
- immutable source versions and labeled history;
- explicit rendered publications;
- rendering in the browser or local companion and agent work in the user's
  attached runner, with server-owned maintenance in background workers;
- ownership transfer and document/account deletion;
- a low-cost single-machine installation;
- migration from local services to managed infrastructure;
- database and blob backups that can be restored and tested independently.

The initial release does not require uninterrupted failover, cross-region
active/active service, exact customer storage billing, cross-document blob
deduplication, or recovery of every unacknowledged in-flight request.

## 3. Explicit removals from catalog v2

The implementation shall remove these catalog v2 mechanisms:

- the twelve-table constraint;
- SQLite and the single-connection catalog executor;
- deployment-wide SQL serialization;
- source chunks and chunk recipes;
- locator-bearing source-tree envelopes;
- transitive checkpoint object closures;
- `checkpoint_objects` and its cached reference counters;
- ordinary object read leases and their heartbeats;
- allocated/available/deleting object states;
- physical-byte reservations and settlement accounting;
- the polymorphic `operations` table;
- durable operation plans and generic recovery cursors;
- custom catalog snapshot coordination and GC freezes;
- exact per-owner physical-byte attribution;
- object deletion as part of normal version retention;
- compatibility shims for v1 or v2 catalogs.

Removal means deleting the production types, SQL, recovery paths, configuration,
status fields, tests, and user-facing errors associated only with these
mechanisms. Renaming them or moving their state into JSON does not satisfy this
requirement.

## 4. Deployment profiles

### 4.1 Single-machine profile

The supported low-cost profile runs these services on one machine:

```text
reverse proxy
LibrePaper application and worker
PostgreSQL
local object directory or remote S3-compatible bucket
```

PostgreSQL should listen on a Unix socket or loopback interface. It must not be
published to the internet. The installer may support a system PostgreSQL
package or a Docker Compose deployment. LibrePaper does not embed or supervise
the PostgreSQL server.

The recommended small production configuration uses local PostgreSQL and a
remote S3-compatible bucket. This keeps large durable content off the server
disk while retaining a one-machine compute footprint.

The minimum documented starting size is two virtual CPUs, 4 GiB of memory, and
SSD storage. A 2 GiB host may be used for light personal deployments with lower
room, worker, and connection limits.

### 4.2 Managed profile

The hosted profile may use:

- one or more stateless HTTP application processes;
- one or more background worker processes;
- managed PostgreSQL with connection pooling and point-in-time recovery;
- S3-compatible object storage;
- a load balancer with document-keyed WebSocket affinity.

Application and worker binaries use the same schema and blob interface as the
single-machine profile. Moving PostgreSQL or blobs changes configuration only.

### 4.3 Configuration

The baseline configuration is conceptually:

```yaml
database:
  url: postgresql:///librepaper
  max_connections: 20

objects:
  type: filesystem
  path: /var/lib/librepaper/objects
```

An S3-compatible configuration supplies endpoint, region, bucket, and a
credential source. Secrets may come from environment variables, files, or the
platform's workload identity. They must not be stored in PostgreSQL.

## 5. PostgreSQL conventions

Entity identifiers generated by LibrePaper are UUIDv7 values. Ordered stream
positions and local sequences may use PostgreSQL `bigint` identities or
document-scoped counters. Public slugs remain separate mutable labels. Digests
are SHA-256 values stored as `bytea`; APIs encode them as lowercase hexadecimal.
Time uses `timestamptz`. Byte lengths use nonnegative `bigint`. User-controlled
JSON is bounded by the application before insertion.

Every schema change is an ordered SQL migration. Startup checks the migration
version and refuses an unsupported newer database. Migrations run through a
dedicated administrative command or under a PostgreSQL advisory lock before
normal service admission. Application processes do not issue runtime DDL.

The Rust implementation shall use an asynchronous PostgreSQL driver and a
bounded connection pool. SQL transactions must not span blob I/O, rendering,
compression, hashing, or network calls.

Database constraints enforce local row relationships and uniqueness. Product
code may use optimistic concurrency for source and publication heads. The
system does not attempt to make PostgreSQL and blob storage one atomic system.
Every database row that references a completed blob is inserted only after the
application has verified that blob's key, digest, and byte length.

## 6. Baseline relational schema

The exact migrations are normative once implemented. The following logical
schema defines the required ownership and relationships. Additional ordinary
indexes, migration metadata, and narrowly scoped tables are allowed when they
make a measured query or product concept clearer. Table count is not a goal.

### 6.1 Accounts and access

```sql
create table accounts (
    id uuid primary key,
    kind text not null check (kind in ('registered', 'anonymous', 'system')),
    provider text,
    provider_subject text,
    handle text not null,
    display_name text not null,
    email text,
    status text not null check (status in ('active', 'erasing', 'blocked')),
    session_generation bigint not null default 1,
    preferences jsonb not null default '{"version":1}'::jsonb,
    created_at timestamptz not null default now(),
    last_seen_at timestamptz not null default now(),
    check (
      (kind = 'registered' and provider is not null and provider_subject is not null)
      or
      (kind <> 'registered' and provider is null and provider_subject is null)
    )
);

create unique index accounts_provider_subject
    on accounts(provider, provider_subject)
    where kind = 'registered';

create table documents (
    id uuid primary key,
    slug text not null unique,
    owner_id uuid not null references accounts(id),
    ownership_mode text not null check (ownership_mode in ('owned', 'open', 'example')),
    title text not null,
    title_key text not null,
    status text not null check (status in ('active', 'deleting')),
    source_format text not null check (source_format in ('markdown','html','typst','latex','quarto')),
    main_path text not null,
    update_sequence bigint not null default 0,
    uncompacted_update_count bigint not null default 0,
    uncompacted_update_bytes bigint not null default 0,
    project_generation bigint not null default 0,
    current_version_id uuid,
    current_publication_id uuid,
    settings jsonb not null default '{"version":1}'::jsonb,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    deleted_at timestamptz
);

create table grants (
    document_id uuid not null references documents(id) on delete cascade,
    account_id uuid not null references accounts(id) on delete cascade,
    role text not null check (role in ('reader','commenter','editor')),
    created_at timestamptz not null default now(),
    primary key (document_id, account_id)
);

create table share_links (
    id uuid primary key,
    document_id uuid not null references documents(id) on delete cascade,
    role text not null check (role in ('reader','commenter','editor')),
    token_hash bytea not null unique,
    label text not null default '',
    generation bigint not null default 1,
    created_at timestamptz not null default now(),
    expires_at timestamptz,
    revoked_at timestamptz
);
```

The application derives `title_key` by Unicode normalization, trimming, and
case folding for search and sorting. Titles are display labels and need not be
unique within an account. Document IDs and slugs provide identity.

A share token is a random bearer secret. PostgreSQL stores only its hash; there
is no encrypted token to reseal and no key-rotation workflow. The owner may
create a replacement link and revoke the old one.

The circular current-version and current-publication foreign keys on
`documents` are added after their target tables are created. They use
`on delete set null` or explicit deletion ordering as selected by the final
migration.

### 6.2 Annotations

```sql
create table annotations (
    id uuid primary key,
    document_id uuid not null references documents(id) on delete cascade,
    kind text not null check (kind in ('comment','highlight','suggestion')),
    body text not null,
    author_account_id uuid references accounts(id),
    author_key text not null,
    author_label text not null,
    selector jsonb not null,
    context jsonb not null default '{"version":1}'::jsonb,
    source_version_id uuid,
    source_update_sequence bigint,
    source_project_generation bigint,
    source_state_vector bytea,
    publication_id uuid,
    proposed_text text,
    suggestion_state text check (suggestion_state in ('proposed','accepted','rejected')),
    resolved_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    check (
      (kind = 'suggestion' and proposed_text is not null and suggestion_state is not null)
      or
      (kind <> 'suggestion' and proposed_text is null and suggestion_state is null)
    )
);

create index annotations_timeline
    on annotations(document_id, created_at, id);

create index annotations_publication_timeline
    on annotations(document_id, publication_id, created_at, id)
    where publication_id is not null;

create table replies (
    id uuid primary key,
    annotation_id uuid not null references annotations(id) on delete cascade,
    author_account_id uuid references accounts(id),
    author_key text not null,
    author_label text not null,
    body text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
```

Historical source-version and publication IDs are descriptive provenance; they
need not remain live foreign keys. Source-dependent annotations may also retain
the captured update sequence, project generation, and bounded Yjs state vector.
A suggestion normally rebases from that captured causal state onto the current
CRDT. It requires exact equality only when its operation replaces the entire
project or depends on unchanged project structure. Keeping an unresolved
annotation does not pin a source version forever.

`author_key` is a server-derived stable pseudonymous actor identifier used for
ownership and attribution checks when no registered account is present. It is
derived from authenticated session or visitor identity and is never accepted
as a client assertion or treated as an access credential.

### 6.3 Assets and source versions

```sql
create table document_assets (
    id uuid primary key,
    document_id uuid not null references documents(id) on delete cascade,
    storage_key text not null unique,
    digest bytea not null,
    byte_length bigint not null check (byte_length >= 0),
    media_type text not null,
    original_name text,
    created_at timestamptz not null default now(),
    unique (document_id, digest, byte_length)
);

create table document_versions (
    id uuid primary key,
    document_id uuid not null references documents(id) on delete cascade,
    sequence bigint not null,
    parent_id uuid,
    through_update_sequence bigint not null,
    project_generation bigint not null,
    archive_key text not null unique,
    archive_encoding_version smallint not null,
    archive_digest bytea not null,
    archive_bytes bigint not null check (archive_bytes >= 0),
    logical_bytes bigint not null check (logical_bytes >= 0),
    reason text not null,
    label text,
    author_account_id uuid references accounts(id),
    author_label text not null,
    created_at timestamptz not null default now(),
    unique (document_id, sequence),
    foreign key (parent_id) references document_versions(id)
);
```

An asset row describes a completed immutable blob. Uploading code writes and
verifies the blob before inserting the row. A process crash before insertion
may leave an orphan blob. That is acceptable and is handled by lifecycle
cleanup; it is not represented by an allocation row.

Assets belong to one document. Identical bytes within a document may reuse the
existing asset row. Cross-document deduplication is not performed. Assets are
retained until their document is deleted, whether or not current or retained
versions still mention them.

Each source version has exactly one compressed source archive. The archive
contains the current small text files and an asset manifest. There is one
database row per version, not one row per version/file or version/object.

### 6.4 Publications

```sql
create table publications (
    id uuid primary key,
    document_id uuid not null references documents(id) on delete cascade,
    source_version_id uuid references document_versions(id),
    request_key text not null,
    request_digest bytea not null,
    manifest_key text not null unique,
    manifest_digest bytea not null,
    publisher_account_id uuid references accounts(id),
    publisher_label text not null,
    created_at timestamptz not null default now(),
    unique (document_id, request_key)
);

create table publication_files (
    publication_id uuid not null references publications(id) on delete cascade,
    path text not null,
    storage_key text not null,
    digest bytea not null,
    byte_length bigint not null check (byte_length >= 0),
    media_type text not null,
    primary key (publication_id, path)
);
```

Publication files are direct immutable outputs, independent of source assets.
The current publication pointer determines what readers may fetch. Old
publication blobs may be retained for a configured grace period and then
deleted by a simple job. Historical publication metadata may be deleted with
the files or retained without promising continued download availability.

### 6.5 Durable collaboration state

```sql
create table document_updates (
    id bigint generated always as identity primary key,
    document_id uuid not null references documents(id) on delete cascade,
    update_sequence bigint not null,
    update_bytes bytea not null,
    created_at timestamptz not null default now(),
    unique (document_id, update_sequence)
);

create table document_bases (
    document_id uuid primary key references documents(id) on delete cascade,
    base_id uuid not null unique,
    through_update_sequence bigint not null,
    project_generation bigint not null,
    snapshot_key text not null,
    snapshot_digest bytea not null,
    snapshot_bytes bigint not null check (snapshot_bytes >= 0),
    previous_snapshot_key text,
    previous_delete_after timestamptz,
    check ((previous_snapshot_key is null) = (previous_delete_after is null)),
    updated_at timestamptz not null default now()
);
```

Updates are normal PostgreSQL rows and remain small and bounded. A room batches
updates for a short interval, locks the document row, increments
`documents.update_sequence`, and inserts the batch under that sequence in one
transaction. `project_generation` advances only when a file is added, removed,
or renamed, or when source format or main path changes. Acknowledgment means
the transaction committed. Update IDs
supplied by clients are handled by the CRDT protocol or a narrow unique key; a
general operations receipt is not created for each update.

A compaction worker reconstructs a document through update sequence `N`, writes
one new compressed Yrs snapshot, and conditionally updates `document_bases`
only when its existing `through_update_sequence < N`. The same transaction
deletes covered update rows and moves the formerly current key into
`previous_snapshot_key` with a deletion deadline. This monotonic comparison
prevents a stale worker from moving the base backward. A failed write may leave
an orphan snapshot. A failed database transaction leaves the old base and
updates valid.

Maintenance keeps the current base and at most one predecessor during the
grace window. It deletes an expired predecessor and sweeps the document's
collaboration prefix for older keys that are neither current nor the protected
predecessor. The baseline key contains `base_id`; no undefined collaboration
epoch participates in its identity.

The application enforces maximum update bytes, maximum uncompacted bytes, and
maximum updates per document. The document row holds the uncompacted count and
byte total so admission is O(1); append advances them in the update transaction,
and compaction recomputes them from the uncovered tail in its transaction. When a document exceeds a soft threshold it
queues compaction. At the hard durable threshold the browser and in-memory CRDT
continue accepting local keystrokes, but the room reports degraded durability,
buffers only within its existing memory bounds, and retries persistence after
compaction. Once that bounded buffer fills, the server refuses further remote
updates and clients retain their unacknowledged changes locally for reconnect;
it does not silently discard accepted edits.

### 6.6 Jobs

```sql
create table jobs (
    id uuid primary key,
    kind text not null,
    document_id uuid references documents(id) on delete cascade,
    account_id uuid references accounts(id) on delete cascade,
    scope_key text not null,
    dedupe_key text,
    payload jsonb not null,
    status text not null check (status in ('queued','running','succeeded','failed','cancelled')),
    priority smallint not null default 0,
    attempts integer not null default 0,
    max_attempts integer not null default 10,
    run_after timestamptz not null default now(),
    locked_by text,
    locked_at timestamptz,
    claim_token uuid,
    last_error text,
    result jsonb,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create unique index jobs_dedupe
    on jobs(kind, scope_key, dedupe_key)
    where dedupe_key is not null and status in ('queued','running');

create index jobs_claim
    on jobs(priority desc, run_after, created_at)
    where status = 'queued';
```

Workers claim bounded batches with `for update skip locked` and assign a fresh
random `claim_token`. A job claim expires when `locked_at` is older than the
kind's timeout. Every progress and completion update compares both worker and
claim token, so a timed-out worker cannot finalize after a replacement claims
the job. Retrying a job must be safe; the final domain update also checks the
relevant monotonic sequence, generation, or current pointer. Jobs contain small
plans and references, never source archives or large results.

`scope_key` is a canonical non-secret string such as `document:<uuid>`,
`account:<uuid>`, or `deployment`. It makes active deduplication work for global
and account jobs without relying on nullable-column uniqueness. A recovery
query requeues expired `running` rows only after replacing their claim token.

Active dedupe keys identify a concrete work generation, for example
`compact:<through-update-sequence>` or
`publication-cleanup:<publication-id>`. Completed rows do not reserve a dedupe
key and are pruned after their result-retention period. Externally visible
idempotency belongs on the domain result row, such as
`publications.request_key`.

The baseline job kinds are source compaction, publication cleanup, document
deletion, account deletion, and maintenance. Each kind has a typed Rust payload
and handler. The table is transport, not a generic workflow language. Multi-step
jobs express their small progress in their own payload or result only when
restart from that point materially saves work.

Interactive rendering and agent execution are deliberately outside this queue.
The browser or local companion renders source and keeps generated output
transient; an attached user runner owns its bounded task queue and local
reconciliation journal. The server only relays bounded, authenticated messages
and stores expiring input objects. Treating either client as a PostgreSQL worker
would make a disconnected client hold a database lease and would move
user-funded compute onto the operator. Any future server-rendered export or
server-hosted agent is server-owned durable work and must use `jobs`.

## 7. Source archive format

The source archive is a versioned `tar.zst` stream whose uncompressed tar
content has deterministic entry ordering and metadata. It contains:

```text
manifest.json
files/<normalized project path>     # small UTF-8 text files
```

`manifest.json` contains:

```json
{
  "version": 1,
  "source_format": "quarto",
  "main_path": "paper.qmd",
  "files": [
    {
      "path": "paper.qmd",
      "kind": "inline",
      "digest": "...",
      "bytes": 12034
    },
    {
      "path": "figures/plot.png",
      "kind": "asset",
      "asset_id": "...",
      "digest": "...",
      "bytes": 483920,
      "media_type": "image/png"
    }
  ]
}
```

Paths are normalized relative UTF-8 paths. Absolute paths, traversal, NUL,
duplicate normalized paths, and platform-specific separators are rejected.
Entries are sorted by normalized path bytes. Tar timestamps, users, groups,
permissions, and other incidental metadata are fixed so the same logical
source produces the same uncompressed tar bytes. Each manifest entry records
the digest of its logical file content; no self-referential whole-archive
logical digest is required.

The zstd encoder, level, and options belong to the archive encoding version.
`archive_digest` and `archive_bytes` describe the exact compressed artifact
that was stored. LibrePaper does not promise identical compressed bytes across
different encoding versions or dependency upgrades. A reader chooses the
decoder from `archive_encoding_version`, verifies the physical archive digest,
and verifies every extracted file against the manifest.

UTF-8 source-like files up to a configurable threshold are stored inline. The
initial threshold is 1 MiB per file. Binary files and larger files are stored
as assets regardless of filename. The total uncompressed inline source remains
bounded by the existing document source limit. The archive has compressed and
uncompressed size limits and a maximum file count.

An asset reference repeats its digest and byte length so archive verification
does not trust a mutable database label. `documents.source_format` and
`documents.main_path` are authoritative for the current project. A historical
archive records the values captured for that version. When opening history,
the archive values describe that historical version; when restoring it, the
restore explicitly updates the document values and project generation. A
mismatch between a version row and its archive is corruption and fails the
read. Opening a version loads the archive, validates its manifest and paths,
and fetches referenced assets. There are no recursive recipes or nested
storage locators.

The current live room may construct archives from its CRDT state and current
asset map. Creating a version does not duplicate asset bytes. Restoring a
version replaces the live source state with the archive contents and referenced
assets, advances `update_sequence` and `project_generation` as applicable, and
records a new version rather than mutating the historical version.

## 8. Write protocols

### 8.1 Asset upload

1. Authenticate and authorize the editor.
2. Apply request size, media type, document asset, account upload-rate, and
   deployment concurrency limits.
3. Stream the body to a new temporary blob key while hashing and counting it.
4. Reject and delete the temporary object if a limit or digest check fails.
5. Move or copy it to an immutable document key when required by the backend.
6. Insert `document_assets` with `on conflict (document_id, digest,
   byte_length) do nothing`, then select the winning row. Concurrent identical
   uploads return the same asset identity. A redundant uploaded blob becomes
   an ordinary orphan for later cleanup.
7. Return the asset ID only after the row commits.

If the process dies before step 6, lifecycle cleanup may delete the unregistered
temporary object. No capacity reservation or recovery operation is created.

### 8.2 Source version

1. Capture a consistent room state, update sequence, project generation, and
   Yjs state vector.
2. Build and verify the deterministic archive outside a transaction.
3. Write the archive under a new immutable key.
4. Begin a PostgreSQL transaction.
5. Lock the document row. Creating historical version metadata does not require
   the captured update sequence to remain current. Moving the current-version
   pointer requires either that the captured sequence is still current or that
   the caller explicitly intends to make this captured state the new head.
6. Insert the version with a document-scoped sequence.
7. Update `documents.current_version_id` when this version is the new head.
8. Commit and return the version.

A blob left by a failed final transaction is harmless. A periodic orphan sweep
or storage lifecycle rule removes it after a minimum seven-day grace period.

### 8.3 Publication

The client or renderer uploads publication files under a temporary publication
prefix. The finalize request includes paths, digests, byte lengths, media types,
the source version, and the expected current publication.

The server verifies uploaded objects, creates a manifest, and writes it before
opening the final transaction. The transaction locks the document, compares
the request's expected current publication, generates the publication UUID,
inserts `publications` and `publication_files`, and moves the current pointer.
The expectation is not persisted after commit. A retry supplies the same
document-scoped `request_key` and returns the existing publication row. Reusing
that key with a different request digest is a conflict.

The previous publication remains readable during preparation. A failed finalize
does not change it. Temporary uploads expire through bucket lifecycle rules or
a cleanup job. No general prepared operation or object lease is required.

### 8.4 Suggestions and agent writes

A suggestion or agent captures a source version or Yjs state vector, the
document update sequence, and project generation. Its final source mutation
normally expresses changes against the current CRDT and rebases from the
captured causal state. It requires an unchanged project generation for changes
whose paths or main-file identity depend on that structure. Whole-project
replacement requires an explicit current-sequence comparison. Successful
mutation, suggestion state, new updates, and generation changes commit in the
smallest applicable PostgreSQL transaction.

Large agent inputs and outputs use temporary blob keys referenced by the job.
They expire after the job retention window. Cancellation sets the job status or
a narrow cancellation timestamp. Workers check cancellation between bounded
steps.

## 9. Realtime collaboration and horizontal processes

A room is an in-memory performance optimization. PostgreSQL updates and the
latest base are durable state. Losing a process disconnects clients; reconnect
reconstructs the room from the base plus updates.

The single-machine profile needs no cross-process room protocol. A horizontally
scaled deployment configures the load balancer to hash WebSocket connections by
document ID so one application process normally owns a document room. HTTP
requests remain stateless.

Room ownership is not a durable product invariant. During failover, two
processes may briefly accept updates. They allocate sequences through
PostgreSQL and converge through the CRDT. Each process polls or receives a
lightweight notification that new update rows exist. PostgreSQL `NOTIFY` may be
used as a wake-up hint; durable delivery always comes from `document_updates`.

If database notification traffic becomes a measured bottleneck, introduce
Redis, NATS, or a dedicated collaboration service. It is not a baseline
dependency.

## 10. Retention and deletion

The default version policy is deterministic. It always retains the current
version and every labeled version. It retains an ordinary version when that
version is either among the newest 50 ordinary versions or is less than 30 days
old. An ordinary version becomes eligible for deletion only when it is
neither current nor labeled, is older than 30 days, and is outside the newest
50. Operators may lower either ordinary-history bound.

A separate ceiling of 1,000 retained versions per document prevents metadata
abuse. When labeled versions alone reach that ceiling, creation of another
labeled version is refused until the user removes a label or deletes a version.
The ceiling never silently deletes a protected version.

Deleting a version deletes its PostgreSQL row and eventually deletes its source
archive. It never deletes document assets. There is no asset reference scan and
no asset garbage collection during version retention.

All completed `document_assets` rows count toward the document's retained-asset
quota, including assets no longer present in the current source or any retained
version. Admission uses the sum of retained asset bytes plus the incoming asset
size. Replacing or dereferencing an asset does not restore quota. The baseline
way to reclaim these bytes is document deletion; an explicit unused-asset purge
may be designed later if measurements justify its additional reference scan.

Deleting a document is a soft-delete transaction followed by a job:

1. Set `status='deleting'` and `deleted_at`.
2. Refuse new writes and stop serving private source.
3. After the configured recovery grace period, delete all blob prefixes owned
   by the document in bounded pages.
4. Delete the document row and cascading relational content.

The worker retries failed blob deletions. Extra retained bytes are acceptable.
An administrator may restore the row during the grace period if the blobs still
exist.

Account deletion transfers or queues deletion of owned documents according to
the product decision, removes grants and identifying attribution, and deletes
the account after dependent jobs finish. It does not require a generic erasure
operation cursor; bounded jobs may enqueue one child document-deletion job per
document.

## 11. Cost controls

Cost control must prevent unbounded external work without simulating the
provider invoice inside the catalog.

The application enforces:

- maximum request bodies before or while reading them;
- maximum inline source bytes and file count per document;
- maximum individual assets and aggregate retained document asset bytes;
- maximum publication files and total publication bytes;
- maximum retained versions per document;
- per-account upload and version-creation rates;
- deployment-wide concurrent uploads and server-owned jobs, plus bounded
  renderer waits and agent relay work per application process;
- bounded PostgreSQL pool size and job claim batches;
- bounded live rooms, sockets, and outbound queue memory;
- temporary-object expiration;
- a global read-only/emergency mode;
- provider billing alerts and, where available, hard spending caps.

The per-document retained-asset limit is enforced from completed
`document_assets` rows at upload finalization. Account and deployment usage
shown to users may be delayed and approximate. A periodic query computes all
retained source archives, all retained assets, retained publication files, and
temporary-upload estimates by account. Admission may use a cached account or
deployment total with conservative margins. The system does not reserve the
maximum possible blob size before upload and does not promise exact byte-level
billing attribution.

The initial defaults retain the existing 4 MiB source-text, 32 MiB document
input-asset, 100 MiB owner, and 5 GiB deployment guidance where those values
remain useful. The deployment limit becomes an operator policy threshold, not
a claim that PostgreSQL and blob bytes have been transactionally reconciled.

Metrics expose actual PostgreSQL database size, blob-provider bucket usage,
temporary bytes, job backlog, upload bytes, download bytes, rendering duration,
and application resource use. Operators compare provider metrics with
LibrePaper estimates and receive an alert on material divergence.

## 12. Blob-store behavior

The blob interface supports:

- streaming immutable put;
- streaming get and ranged get;
- head with byte length and provider version/ETag;
- delete;
- bounded prefix listing;
- optional server-side copy;
- optional signed download/upload URLs.

Filesystem keys live below one configured root and are protected against path
traversal. Writes use a temporary file, sync according to operator policy, and
rename atomically into place. S3-compatible writes use unique keys and do not
overwrite live objects.

Object namespaces are simple and inspectable:

```text
documents/<document-id>/assets/<asset-id>
documents/<document-id>/versions/<version-id>.tar.zst
documents/<document-id>/collaboration/<base-id>.yrs.zst
documents/<document-id>/publications/<publication-id>/<path>
temporary/<document-id>/<random-id>
jobs/<job-id>/<random-id>
```

The database stores complete keys. Slug changes do not move objects. All writes
use new keys. Provider bucket versioning is recommended but not required.

The maintenance worker scans `documents/` and `temporary/` in bounded,
lexicographic pages. It stores only one opaque cursor per namespace in a small
`maintenance_cursors` table. For document objects, one `ANY(text[])` query over
the asset, version, publication, publication-file, and collaboration-base rows
identifies references in that page. Unreferenced objects older than seven days
are deleted. Temporary objects use the provider or filesystem modification
time; final UUIDv7 object names provide a conservative timestamp fallback when
a backend omits modification time. A completed pass resets its cursor. This is
an eventual orphan sweep over ordinary domain references, not a stored closure
graph or exact byte-accounting ledger.

## 13. Backups and restore

PostgreSQL and immutable blobs are backed up independently. They do not require
a global write freeze.

For the single-machine profile:

1. Record the backup start time and database migration version.
2. Open a repeatable-read, read-only PostgreSQL transaction and export its
   snapshot with `pg_export_snapshot()`.
3. Run `pg_dump --format=custom --snapshot=<snapshot-id>` while the exporting
   transaction remains open.
4. Query the complete set of referenced blob keys and expected metadata through
   that same exported snapshot and write it as the backup object manifest.
5. Copy the dump outside the primary machine.
6. If blobs are local, copy every manifest key to the same remote destination.
7. Verify that every referenced key is present with the recorded length and
   digest; any missing or unreadable key fails the backup.
8. Record checksums, start and completion times, migration version, reference
   counts, and verification results in the completion manifest.
9. Retain several generations according to operator policy.

A database dump may omit a concurrently uploaded blob that has not yet gained a
row, which is harmless. The dump fixes the reference set, so continuous new
writes cannot make backup copying unbounded. Version archives, collaboration
bases, and publication files are not physically deleted until a grace period
longer than the supported maximum backup duration has elapsed. This ensures a
key referenced by the snapshot remains available for step 5. Extra blobs are
harmless and need not be copied. Remote object storage supplies its own
durability/versioning and does not need to be copied for every database backup
unless the operator's disaster model requires a second provider.

Managed deployments use provider point-in-time recovery plus object versioning
or replication. LibrePaper documents these facilities but does not reproduce
them in application code.

Restore always targets an empty database and blob namespace. It restores the
database, checks migrations and relational integrity, verifies a sample or the
complete set of referenced current blobs according to operator choice, then
starts workers before admitting traffic. A restore may contain harmless extra
blobs.

The project export feature is separate from disaster recovery. It emits a
portable archive of source, assets, annotations, and selected publications for
one document.

## 14. Security

PostgreSQL uses a dedicated role with access only to the LibrePaper database.
Network deployments require TLS; same-machine deployments prefer a protected
Unix socket. Database and object credentials are external secrets.

Private blobs are not exposed through permanent public URLs. LibrePaper either
streams them after authorization or issues short-lived signed URLs. Published
content remains isolated on the document origin under the existing content
security policy. Signed URLs must be scoped to one exact object and short
lived.

Share tokens contain at least 128 bits of cryptographically secure random
entropy and are stored only as SHA-256 hashes. Plain SHA-256 is appropriate for
these high-entropy bearer tokens; they are not human-selected passwords and
must not be changed to an expensive password KDF without revisiting request
denial-of-service costs. Session and CSRF protections remain application
concerns. User-controlled archive paths, media types, sizes, compressed
expansion, and file counts are validated before extraction or rendering.

## 15. Observability

LibrePaper reports at least:

- PostgreSQL pool usage, acquisition latency, query latency, and errors;
- transaction retries and lock waits;
- update rows and uncompacted bytes per active document;
- room reconstruction and compaction duration;
- job counts by kind/status, claim latency, attempts, and oldest age;
- blob put/get/delete counts, bytes, latency, and errors;
- temporary and orphan cleanup counts;
- source-version and publication creation latency;
- current database and bucket size;
- HTTP, WebSocket, room, renderer, and agent resource limits.

Metrics must use bounded labels. Document, account, object, and job IDs do not
become metric labels. Logs may carry them under the deployment's privacy and
retention policy.

## 16. Failure behavior

The design deliberately accepts these outcomes:

| Failure | Outcome |
|---|---|
| Process dies during blob upload | Temporary or orphan blob is cleaned later |
| Blob succeeds, database commit fails | Unreferenced immutable blob is cleaned later |
| Worker dies | PostgreSQL job lease expires and another worker retries |
| Room process dies | Clients reconnect and room rebuilds from base plus updates |
| Compaction blob succeeds, transaction fails | Old base and updates remain authoritative |
| Publication preparation fails | Current publication remains unchanged |
| Old blob deletion fails | Bytes remain and cleanup retries |
| Provider usage estimate drifts | Alert and conservative admission; no ledger repair protocol |
| Single machine is lost | Restore PostgreSQL backup and remote/local blob backup |

The application prioritizes acknowledged user-visible state. It does not spend
comparable complexity recovering unacknowledged staging work.

## 17. Implementation plan

Because catalog v2 is not a released persistence format, implementation is a
hard break. It creates a fresh PostgreSQL database and removes SQLite support,
the v2 object model, and their production module declarations before porting
call sites. Compile errors are the cutover inventory. There is no catalog v2
converter, fallback backend, dual-write interval, or public compatibility API.

The implementation boundary is a product operation rather than an old catalog
method. Commands such as document creation, version commit, publication,
sharing, annotation mutation, and agent source application own their complete
PostgreSQL transaction. Focused query modules serve bounded reads. The final
implementation is not required to reproduce any catalog v2 method whose work
is subsumed by one of these operations.

### Phase A: foundation

- Add the asynchronous PostgreSQL driver and pool.
- Add migrations for accounts, documents, access, annotations, and jobs.
- Replace catalog startup and configuration.
- Port authentication, document listing, sharing, annotations, and comments.
- Establish transaction helpers and typed row/domain conversions without a
  general repository framework.

Gate: relevant HTTP and authorization tests pass against a real temporary
PostgreSQL instance. No compiled production module depends on SQLite or the v2
catalog, even while later workflows are temporarily incomplete on the branch.

### Phase B: simple source storage

- Implement deterministic source archives.
- Implement immutable document assets.
- Replace chunks, recipes, tree envelopes, closure admission, and reconstruction.
- Port version creation, history reads, restore, suggestions, and agent source
  application.
- Retain assets until document deletion.

Gate: all five source formats round-trip projects containing text and binary
assets; repeated versions upload no unchanged asset bytes.

### Phase C: collaboration and jobs

- Store batched Yrs updates in PostgreSQL.
- Write and activate collaboration bases.
- Claim compaction, cleanup, maintenance, and deletion work through `jobs`.
- Keep transient browser rendering and attached-runner agent execution on their
  bounded authenticated channels; they do not claim PostgreSQL jobs.
- Implement collaboration directly on PostgreSQL; the catalog executor,
  operation receipts, leases, and v2 journal have already been removed at the
  hard-break boundary.

Gate: process-kill tests recover acknowledged edits and retry jobs without
duplicating user-visible effects.

### Phase D: publication, cleanup, and backup

- Port publication staging and activation.
- Implement temporary-prefix and old-publication cleanup.
- Implement document deletion.
- Add `pg_dump`-based backup commands and restore verification.
- Add S3-compatible storage and signed delivery if not completed earlier.

Gate: a same-machine deployment and a managed-style deployment pass the same
application suite; backup restore produces readable current documents and
publications.

### Phase E: deletion and documentation

- Delete any unreferenced catalog v2 source files, tests, configuration, status
  fields, and documentation left in the worktree after the initial compile
  cutover.
- Update installation, hosting, backup, quota, privacy, and troubleshooting
  documentation.

Gate: searches find no production reference to catalog v2 tables, object
states, closures, recipes, or SQLite.

## 18. Testing strategy

Tests use PostgreSQL, not an SQLite approximation. Unit tests cover the
deterministic uncompressed archive layout, versioned compression, and domain
validation. Integration tests create isolated
databases or schemas and exercise real transactions.

Required scenarios include:

- concurrent document creation with duplicate titles and ownership transfer;
- grant and link revocation during reads and writes;
- source archive round-trip for all formats;
- unchanged large image reused across many versions;
- large binary never embedded in each archive;
- process death before and after blob write and database commit;
- concurrent version commits from the same captured update sequence;
- room reconstruction after abrupt process termination;
- compaction concurrent with new updates;
- multiple workers claiming jobs without duplicate claims;
- expired job claim recovery;
- publication conflict and retry;
- deletion with partial blob failures;
- quota/rate/concurrency refusal at every boundary;
- database backup during writes and restore into an empty deployment;
- local filesystem and S3-compatible blob contract tests;
- authorization on conditional and ranged blob responses.

Fault tests distinguish acknowledged state from temporary work. They do not
require cleanup to be immediate; they require eventual bounded convergence and
continued readability of committed current state.

## 19. Performance and cost acceptance

Before release, record reproducible measurements for:

- 1, 20, and 100 concurrent active rooms;
- update append and room reconstruction at p50, p95, and p99;
- version creation for 100 KiB, 1 MiB, and 4 MiB inline source;
- versions containing 1, 100, and 512 reused assets;
- document listing and annotation timelines at product limits;
- job claim throughput with 1, 4, and 16 workers;
- publication upload and activation at configured limits;
- PostgreSQL database size for 10,000 documents and 500,000 versions;
- blob bytes and operations for repeated small edits to projects with large
  unchanged images;
- backup duration and restore duration at representative scale.

The release target is no database transaction above 250 ms at documented
ordinary limits, excluding deliberate lock waiting. Interactive p95 database
work should remain below 50 ms on the reference small deployment. Room update
acknowledgment p95 should remain below 100 ms under the documented room load.

The cost report separates fixed compute/database cost, PostgreSQL storage,
object bytes, object operations, transfer, and rendering/agent work. It models
at least 100, 10,000, and 100,000 active documents. Cost protection is accepted
only when provider-side alerts or caps and application refusal tests are both
demonstrated.

## 20. Definition of done

Catalog v3 is complete when:

- PostgreSQL is the only production catalog;
- the same application supports local and managed PostgreSQL by configuration;
- filesystem and S3-compatible blobs pass the same contract suite;
- versions contain one compressed inline-source archive plus direct immutable
  asset references;
- unchanged assets are not uploaded for each version;
- assets are retained until document deletion;
- collaboration survives process loss through PostgreSQL updates and blob
  bases;
- server-owned durable background work uses the PostgreSQL job queue;
- no catalog-wide connection mutex, object ledger, closure graph, read lease,
  allocation reservation, or general operation state machine remains;
- local backup uses PostgreSQL tools and immutable blob copying;
- managed deployment documentation uses provider backup facilities;
- cost and performance measurements meet the stated gates;
- the implementation and operator documentation describe the same system.
