-- A compaction base remains the same snapshot when it is retired. Keep both
-- stages in one table, retaining metadata for backups throughout its grace.
LOCK TABLE document_bases, superseded_bases IN ACCESS EXCLUSIVE MODE;

CREATE TABLE document_snapshots (
    snapshot_key text PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- Legacy superseded rows held only a key and deadline. Their missing
    -- metadata stays NULL; every current snapshot must have complete evidence.
    base_id uuid UNIQUE,
    through_update_sequence bigint CHECK (through_update_sequence >= 0),
    vector bytea,
    snapshot_digest bytea CHECK (octet_length(snapshot_digest) = 32),
    snapshot_bytes bigint CHECK (snapshot_bytes >= 0),
    updated_at timestamptz DEFAULT now(),
    delete_after timestamptz,
    CONSTRAINT document_snapshots_current_metadata CHECK (
        delete_after IS NOT NULL OR (
            base_id IS NOT NULL AND through_update_sequence IS NOT NULL
            AND vector IS NOT NULL AND snapshot_digest IS NOT NULL
            AND snapshot_bytes IS NOT NULL AND updated_at IS NOT NULL
        )
    )
);

CREATE UNIQUE INDEX document_snapshots_current ON document_snapshots(document_id)
    WHERE delete_after IS NULL;
CREATE INDEX document_snapshots_document ON document_snapshots(document_id);
CREATE INDEX document_snapshots_due ON document_snapshots(delete_after)
    WHERE delete_after IS NOT NULL;

INSERT INTO document_snapshots
    (snapshot_key,document_id,base_id,through_update_sequence,vector,
     snapshot_digest,snapshot_bytes,updated_at)
SELECT snapshot_key,document_id,base_id,through_update_sequence,vector,
       snapshot_digest,snapshot_bytes,updated_at
FROM document_bases;

INSERT INTO document_snapshots(snapshot_key,document_id,delete_after,updated_at)
SELECT snapshot_key,document_id,delete_after,NULL FROM superseded_bases;

DROP TABLE superseded_bases;
DROP TABLE document_bases;
