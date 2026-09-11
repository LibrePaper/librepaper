-- Durable graph for the physical representation beneath immutable checkpoint
-- trees.  The tree remains authoritative; these rows only describe how a
-- text digest is stored and which retained checkpoints need it.
CREATE TABLE source_history_encodings (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    file_digest TEXT NOT NULL,
    recipe_key TEXT NOT NULL,
    recipe_digest TEXT NOT NULL,
    codec INTEGER NOT NULL CHECK (codec > 0),
    uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
    recipe_bytes INTEGER NOT NULL CHECK (recipe_bytes >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (storage_id, file_digest)
) WITHOUT ROWID;

CREATE TABLE source_history_objects (
    storage_id TEXT NOT NULL,
    file_digest TEXT NOT NULL,
    object_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    PRIMARY KEY (storage_id, file_digest, object_key),
    FOREIGN KEY (storage_id, file_digest)
        REFERENCES source_history_encodings(storage_id, file_digest)
        ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX source_history_objects_by_object
    ON source_history_objects(storage_id, object_key);

CREATE TABLE source_history_checkpoint_files (
    storage_id TEXT NOT NULL,
    checkpoint_sha TEXT NOT NULL,
    file_digest TEXT NOT NULL,
    PRIMARY KEY (storage_id, checkpoint_sha, file_digest),
    FOREIGN KEY (storage_id, file_digest)
        REFERENCES source_history_encodings(storage_id, file_digest)
        ON DELETE RESTRICT
) WITHOUT ROWID;
CREATE INDEX source_history_checkpoint_files_by_digest
    ON source_history_checkpoint_files(storage_id, file_digest);

-- SQLite removes the parent document before cascading into its checkpoints.
-- At that point the checkpoint trigger cannot resolve OLD.slug to storage_id.
-- Drop the document's graph edges while its stable identity is still known,
-- before the encoding cascade encounters their RESTRICT foreign key.
CREATE TRIGGER source_history_document_removed
BEFORE DELETE ON documents
BEGIN
    DELETE FROM source_history_checkpoint_files WHERE storage_id=OLD.storage_id;
END;

-- A lease is installed before an object upload.  It protects an ambiguous or
-- slow writer from source-history GC; expiration is handled by maintenance,
-- never by assuming an age grace period is sufficient.
CREATE TABLE source_history_write_leases (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    object_key TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= created_at),
    PRIMARY KEY (storage_id, operation_id, object_key)
) WITHOUT ROWID;
CREATE INDEX source_history_write_leases_expiry
    ON source_history_write_leases(expires_at, storage_id);
-- Every GC admission and publication recheck asks whether a physical key is
-- still leased.  Keep that lookup indexed independently of the expiry scan;
-- otherwise a large set of unrelated in-flight writes turns each object
-- decision into an unbounded lease-table scan.
CREATE INDEX source_history_write_leases_by_object
    ON source_history_write_leases(storage_id, object_key);

-- Catalogue-only retention paths also delete checkpoint rows directly.  Keep
-- the graph safe on those paths: queue encoded objects before their encoding
-- rows disappear, and let the existing deletion worker release accounting
-- only after physical deletion is confirmed.
CREATE TRIGGER source_history_checkpoint_removed
AFTER DELETE ON checkpoints
BEGIN
    DELETE FROM source_history_checkpoint_files
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND checkpoint_sha=OLD.sha;
    INSERT INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after)
    SELECT OLD.slug,o.object_key,o.bytes,unixepoch(),unixepoch()
      FROM source_history_objects o
      JOIN source_history_encodings e
        ON e.storage_id=o.storage_id AND e.file_digest=o.file_digest
     WHERE o.storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       -- An encoded chunk/recipe can be shared by more than one file digest.
       -- It is reclaimable only when no retained checkpoint names *any*
       -- encoding that owns this physical key.
       AND NOT EXISTS (SELECT 1
                      FROM source_history_objects o2
                      JOIN source_history_checkpoint_files r
                        ON r.storage_id=o2.storage_id
                       AND r.file_digest=o2.file_digest
                      WHERE o2.storage_id=o.storage_id
                        AND o2.object_key=o.object_key)
       AND NOT EXISTS (SELECT 1 FROM source_history_write_leases l
                      WHERE l.storage_id=o.storage_id AND l.object_key=o.object_key)
    ON CONFLICT(slug,object_key) DO UPDATE SET
      bytes=excluded.bytes,
      delete_after=MIN(pending_deletes.delete_after,excluded.delete_after);
    DELETE FROM source_history_encodings
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND NOT EXISTS (SELECT 1 FROM source_history_checkpoint_files r
                      WHERE r.storage_id=source_history_encodings.storage_id
                        AND r.file_digest=source_history_encodings.file_digest)
       AND NOT EXISTS (SELECT 1
                      FROM source_history_objects o
                      JOIN source_history_write_leases l
                        ON l.storage_id=o.storage_id
                       AND l.object_key=o.object_key
                      WHERE o.storage_id=source_history_encodings.storage_id
                        AND o.file_digest=source_history_encodings.file_digest);
END;
