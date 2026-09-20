-- One deployment writer fences every semantic document transaction.  The row
-- lock, rather than an unlocked epoch read, remains held until commit.
CREATE TABLE deployment_writer (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    epoch bigint NOT NULL DEFAULT 0 CHECK (epoch >= 0),
    activated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO deployment_writer(singleton) VALUES(true) ON CONFLICT DO NOTHING;

ALTER TABLE documents
    ADD COLUMN commit_sequence bigint NOT NULL DEFAULT 0 CHECK (commit_sequence >= 0),
    ADD COLUMN source_revision bigint NOT NULL DEFAULT 0 CHECK (source_revision >= 0),
    ADD COLUMN source_schema integer NOT NULL DEFAULT 1 CHECK (source_schema >= 1);

-- A revision survives compaction of the update bytes it names.
CREATE TABLE document_source_revisions (
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    source_revision bigint NOT NULL CHECK (source_revision >= 1),
    update_sequence bigint NOT NULL CHECK (update_sequence >= 1),
    frontier bytea NOT NULL,
    schema_version integer NOT NULL DEFAULT 1 CHECK (schema_version >= 1),
    encoding_version integer NOT NULL DEFAULT 1 CHECK (encoding_version >= 1),
    actor_key text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(document_id, source_revision),
    UNIQUE(document_id, update_sequence)
);

-- Receipts are lifetime records for non-idempotent semantic commands.  Source
-- synchronization relies on Loro operation identity and vector coverage.
CREATE TABLE document_command_receipts (
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    principal_key text NOT NULL,
    request_id uuid NOT NULL,
    command_digest bytea NOT NULL CHECK (octet_length(command_digest) = 32),
    status text NOT NULL,
    commit_sequence bigint NOT NULL CHECK (commit_sequence >= 0),
    source_revision bigint NOT NULL CHECK (source_revision >= 0),
    result jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(document_id, principal_key, request_id)
);

-- Original evidence names a committed source revision. Existing rows retain
-- their frontier/checkpoint evidence until the migration audit maps them.
ALTER TABLE annotations ADD COLUMN source_revision bigint;
ALTER TABLE annotations ADD CONSTRAINT annotations_source_revision_nonnegative
    CHECK (source_revision IS NULL OR source_revision >= 0);

CREATE INDEX document_source_revisions_frontier
    ON document_source_revisions(document_id, source_revision DESC);

-- Preserve every exact retained legacy frontier before later compaction can
-- delete its update row. The old update sequence is a stable, monotonic
-- initial revision number; new commits continue above it.
INSERT INTO document_source_revisions
    (document_id,source_revision,update_sequence,frontier,actor_key,created_at)
SELECT document_id,update_sequence,update_sequence,frontier,'migration:legacy-update',created_at
FROM document_updates
WHERE frontier <> ''::bytea
ON CONFLICT DO NOTHING;

UPDATE documents d SET
    source_revision = migrated.last_revision,
    commit_sequence = GREATEST(d.commit_sequence,migrated.last_revision)
FROM (
    SELECT document_id,max(source_revision) AS last_revision
    FROM document_source_revisions GROUP BY document_id
) migrated
WHERE d.id=migrated.document_id;

-- A legacy comment either names an operation-log moment (`frontier:` plus
-- standard base64) or an explicit checkpoint UUID. Map only exact retained
-- frontier/sequence evidence. Never attach an anchor to a nearby revision.
UPDATE annotations a SET source_revision=matched.source_revision
FROM (
    SELECT DISTINCT ON (a.id) a.id,r.source_revision
    FROM annotations a
    JOIN document_source_revisions r ON r.document_id=a.document_id
    LEFT JOIN document_versions v
      ON v.document_id=a.document_id
     AND v.through_update_sequence=r.update_sequence
    WHERE a.checkpoint_id = 'frontier:' ||
              replace(encode(r.frontier,'base64'),E'\n','')
       OR a.checkpoint_id = v.id::text
    ORDER BY a.id,r.source_revision DESC
) matched
WHERE a.id=matched.id;

CREATE TABLE annotation_revision_migration_exceptions (
    annotation_id uuid PRIMARY KEY REFERENCES annotations(id) ON DELETE CASCADE,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    checkpoint_id text NOT NULL,
    reason text NOT NULL CHECK (reason IN
        ('frontier_not_retained','checkpoint_frontier_not_retained','unknown_evidence')),
    recorded_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX annotation_revision_migration_exceptions_document
    ON annotation_revision_migration_exceptions(document_id,reason,annotation_id);

INSERT INTO annotation_revision_migration_exceptions
    (annotation_id,document_id,checkpoint_id,reason)
SELECT a.id,a.document_id,a.checkpoint_id,
       CASE
         WHEN a.checkpoint_id LIKE 'frontier:%' THEN 'frontier_not_retained'
         WHEN EXISTS(SELECT 1 FROM document_versions v
                     WHERE v.document_id=a.document_id AND v.id::text=a.checkpoint_id)
           THEN 'checkpoint_frontier_not_retained'
         ELSE 'unknown_evidence'
       END
FROM annotations a
WHERE a.source_revision IS NULL;

-- Checkpoint identity is relational metadata over a committed source
-- revision. The immutable plain-source archive is attached later by a worker
-- and is never the checkpoint's identity.
CREATE TABLE document_checkpoints (
    id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    sequence bigint NOT NULL CHECK (sequence >= 1),
    parent_id uuid REFERENCES document_checkpoints(id) ON DELETE SET NULL,
    source_revision bigint,
    tree_digest bytea CHECK (tree_digest IS NULL OR octet_length(tree_digest)=32),
    changed_paths text[],
    file_count integer CHECK (file_count IS NULL OR file_count >= 0),
    reason text NOT NULL,
    label text,
    author_account_id uuid REFERENCES accounts(id),
    author_label text NOT NULL,
    archive_status text NOT NULL DEFAULT 'pending'
        CHECK (archive_status IN ('pending','ready','failed')),
    archive_version_id uuid REFERENCES document_versions(id) ON DELETE SET NULL,
    archive_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(document_id,sequence),
    FOREIGN KEY(document_id,source_revision)
        REFERENCES document_source_revisions(document_id,source_revision)
);
CREATE INDEX document_checkpoints_timeline
    ON document_checkpoints(document_id,sequence DESC);
CREATE INDEX document_checkpoints_archive_pending
    ON document_checkpoints(document_id,archive_status,sequence)
    WHERE archive_status <> 'ready';

-- Existing versions remain valid evidence and export artifacts. Where their
-- exact retained revision is known, expose their event metadata through the
-- new checkpoint index without rewriting or duplicating the archive.
INSERT INTO document_checkpoints
    (id,document_id,sequence,parent_id,source_revision,tree_digest,changed_paths,
     file_count,reason,label,author_account_id,author_label,archive_status,
     archive_version_id,created_at)
SELECT v.id,v.document_id,v.sequence,
       CASE WHEN EXISTS(SELECT 1 FROM document_versions p WHERE p.id=v.parent_id)
            THEN v.parent_id ELSE NULL END,
       r.source_revision,v.tree_digest,v.changed_paths,v.file_count,v.reason,v.label,
       v.author_account_id,v.author_label,'ready',v.id,v.created_at
FROM document_versions v
LEFT JOIN document_source_revisions r
  ON r.document_id=v.document_id
 AND r.update_sequence=v.through_update_sequence;
