-- Recovery bases and immutable manifest shards let restart replay a bounded
-- tail instead of scanning the lifetime journal.  Segment ownership is
-- recorded explicitly so compaction can retire a segment only after every
-- covered document has a base.
ALTER TABLE journal_segments ADD COLUMN storage_id TEXT NOT NULL DEFAULT '';
ALTER TABLE journal_segments ADD COLUMN epoch INTEGER NOT NULL DEFAULT 0 CHECK (epoch >= 0);
ALTER TABLE journal_segments ADD COLUMN first_sequence INTEGER NOT NULL DEFAULT 0 CHECK (first_sequence >= 0);
ALTER TABLE journal_segments ADD COLUMN last_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_sequence >= 0);

CREATE INDEX journal_segments_storage_sequence
    ON journal_segments(storage_id, epoch, last_sequence);

-- One immutable segment can contain records for many rooms. Keep coverage
-- separately from the legacy single-owner columns so replay and retirement
-- remain correct when the deployment-wide coordinator batches rooms.
CREATE TABLE journal_segment_coverage (
    segment_id TEXT NOT NULL,
    storage_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch >= 0),
    first_sequence INTEGER NOT NULL CHECK (first_sequence >= 0),
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= first_sequence),
    PRIMARY KEY (segment_id, storage_id, epoch),
    FOREIGN KEY (segment_id) REFERENCES journal_segments(segment_id) ON DELETE CASCADE
);
CREATE INDEX journal_segment_coverage_lookup
    ON journal_segment_coverage(storage_id, epoch, last_sequence);

CREATE TABLE journal_bases (
    base_id TEXT PRIMARY KEY,
    storage_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch >= 0),
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    object_key TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    committed_at INTEGER NOT NULL
);
CREATE INDEX journal_bases_storage_sequence
    ON journal_bases(storage_id, sequence);

CREATE TABLE journal_manifest_shards (
    shard_id TEXT PRIMARY KEY,
    shard_seq INTEGER NOT NULL UNIQUE CHECK (shard_seq >= 0),
    object_key TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    committed_at INTEGER NOT NULL
);
