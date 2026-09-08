-- Upload admission is a rolling-hour operation limit, not a property of a
-- document's current updated_at.  Keep the small counter in SQLite so
-- replacements cannot evade the limit by repeatedly updating one row.
CREATE TABLE upload_buckets (
    owner_kind TEXT NOT NULL,
    owner_value TEXT NOT NULL,
    bucket INTEGER NOT NULL CHECK (bucket >= 0),
    uploads INTEGER NOT NULL CHECK (uploads >= 0),
    PRIMARY KEY (owner_kind, owner_value, bucket)
) WITHOUT ROWID;
CREATE INDEX upload_buckets_recent ON upload_buckets(bucket, owner_kind, owner_value);
