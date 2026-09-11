-- Physical asset references held by immutable checkpoints. A missing row is
-- deliberately not interpreted as an empty asset set: it is a legacy,
-- unmeasured checkpoint that retention must leave conservatively untouched.
CREATE TABLE checkpoint_asset_refs (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    checkpoint_sha TEXT NOT NULL,
    object_key TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    PRIMARY KEY (storage_id, checkpoint_sha, object_key)
) WITHOUT ROWID;
CREATE INDEX checkpoint_asset_refs_by_object
    ON checkpoint_asset_refs(storage_id, object_key);

-- Presence is significant: zero rows in checkpoint_asset_refs is either an
-- explicitly assetless checkpoint or a pre-migration checkpoint. Keep that
-- distinction durable so legacy history is never treated as reclaimable.
CREATE TABLE checkpoint_asset_sets (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    checkpoint_sha TEXT NOT NULL,
    asset_count INTEGER NOT NULL CHECK (asset_count >= 0),
    PRIMARY KEY (storage_id, checkpoint_sha)
) WITHOUT ROWID;

CREATE TRIGGER checkpoint_asset_removed
AFTER DELETE ON checkpoints
BEGIN
    -- Queueing is deliberately not physical deletion. The ledger stays
    -- charged until the deletion worker confirms physical reclamation.
    INSERT INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after)
    SELECT OLD.slug,r.object_key,r.bytes,unixepoch(),unixepoch()
      FROM checkpoint_asset_refs r
     WHERE r.storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND r.checkpoint_sha=OLD.sha
       AND NOT EXISTS (
           SELECT 1
             FROM checkpoint_asset_refs retained
             JOIN checkpoints c ON c.slug=OLD.slug
                              AND c.sha=retained.checkpoint_sha
            WHERE retained.storage_id=r.storage_id
              AND retained.object_key=r.object_key
       )
       AND NOT EXISTS (
           SELECT 1 FROM source_history_write_leases l
            WHERE l.storage_id=r.storage_id AND l.object_key=r.object_key
       )
       -- If the live journal is ahead of the remaining durable checkpoints,
       -- do not even create a pending row: the next publication must be able
       -- to lease this asset. The object ledger remains charged until a later
       -- live-root reconciliation can queue it safely.
       AND COALESCE((
             SELECT MAX(c.last_sequence)
               FROM journal_segment_coverage c
              WHERE c.storage_id=r.storage_id OR c.storage_id=''
       ),0) <= COALESCE((
             SELECT MAX(c.durable_seq)
               FROM checkpoints c
              WHERE c.slug=OLD.slug
       ),0)
       AND COALESCE((
             SELECT MAX(b.sequence)
               FROM journal_bases b
              WHERE b.storage_id=r.storage_id OR b.storage_id=''
       ),0) <= COALESCE((
             SELECT MAX(c.durable_seq)
               FROM checkpoints c
              WHERE c.slug=OLD.slug
       ),0)
    ON CONFLICT(slug,object_key) DO UPDATE SET
      bytes=excluded.bytes,
      delete_after=MIN(pending_deletes.delete_after,excluded.delete_after);
    DELETE FROM checkpoint_asset_refs
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND checkpoint_sha=OLD.sha;
    DELETE FROM checkpoint_asset_sets
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND checkpoint_sha=OLD.sha;
END;
