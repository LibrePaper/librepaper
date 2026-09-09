-- Durable state needed by the local contract.  Keep these tables small and
-- keyed by stable ids so an interrupted worker can resume without replaying
-- object-store side effects.
CREATE TABLE IF NOT EXISTS account_activity (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    last_qualified_at TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS erasure_batches (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    cursor TEXT,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS maintenance_jobs (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('active', 'released', 'failed')),
    reserved_bytes INTEGER NOT NULL CHECK (reserved_bytes >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS maintenance_jobs_status
    ON maintenance_jobs(status, updated_at, id);

CREATE INDEX IF NOT EXISTS checkpoints_cursor
    ON checkpoints(slug, seq, sha);
CREATE INDEX IF NOT EXISTS comments_cursor
    ON comments(slug, seq, id);
CREATE INDEX IF NOT EXISTS replies_cursor
    ON replies(slug, comment_id, created, id);
CREATE INDEX IF NOT EXISTS renderings_cursor
    ON renderings(slug, at, tree_sha);
CREATE INDEX IF NOT EXISTS documents_storage_status
    ON documents(storage_id, status);

INSERT OR IGNORE INTO journal_state
    (id, deployment_id, writer_generation, revision, last_operation_id,
     next_segment_seq, manifest_key, manifest_digest, manifest_length, tail_after)
VALUES (1, '', '', 0, '', 0, '', '', 0, 0);
