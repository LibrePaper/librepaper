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

CREATE TABLE journal_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    deployment_id TEXT NOT NULL,
    writer_generation TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    last_operation_id TEXT NOT NULL,
    next_segment_seq INTEGER NOT NULL CHECK (next_segment_seq >= 0),
    manifest_key TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    manifest_length INTEGER NOT NULL CHECK (manifest_length >= 0),
    tail_after INTEGER NOT NULL CHECK (tail_after >= 0)
);

CREATE TABLE journal_preparations (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
    expected_generation TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    plan TEXT NOT NULL CHECK (length(CAST(plan AS BLOB)) <= 262144),
    resolved_at INTEGER
);
CREATE UNIQUE INDEX journal_preparations_unresolved
    ON journal_preparations (1) WHERE resolved_at IS NULL;

CREATE TABLE journal_segments (
    segment_id TEXT PRIMARY KEY,
    segment_seq INTEGER NOT NULL UNIQUE CHECK (segment_seq >= 0),
    operation_id TEXT NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    committed_at INTEGER NOT NULL
);
CREATE INDEX journal_segments_sequence ON journal_segments (segment_seq);

CREATE TABLE journal_retirements (
    object_key TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    payload_bytes INTEGER NOT NULL DEFAULT 0 CHECK (payload_bytes >= 0),
    maintenance_bytes INTEGER NOT NULL DEFAULT 0 CHECK (maintenance_bytes >= 0),
    retired_revision INTEGER,
    modified_at INTEGER NOT NULL,
    first_unreferenced_at INTEGER,
    delete_after INTEGER NOT NULL
);
CREATE INDEX journal_retirements_due
    ON journal_retirements (delete_after, object_key);
