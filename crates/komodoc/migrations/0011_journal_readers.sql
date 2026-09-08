-- Bounded leases protect immutable journal objects while a recovery reader
-- still has an in-flight object read. Expired leases are disposable state;
-- durable journal references and preparation plans remain authoritative.
CREATE TABLE journal_readers (
    reader_id TEXT PRIMARY KEY,
    object_key TEXT NOT NULL,
    opened_at INTEGER NOT NULL CHECK (opened_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= opened_at),
    heartbeat_at INTEGER NOT NULL CHECK (heartbeat_at >= opened_at)
);
CREATE INDEX journal_readers_object_expiry
    ON journal_readers(object_key, expires_at, reader_id);
