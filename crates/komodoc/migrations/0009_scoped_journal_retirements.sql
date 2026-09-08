-- Retirements are normally attributable to the storage identity whose
-- journal ownership was compacted or erased.  Keeping that identity makes
-- deletion gates local to one document; the empty value is reserved for
-- deployment-wide manifest entries and legacy rows.
ALTER TABLE journal_retirements ADD COLUMN storage_id TEXT NOT NULL DEFAULT '';
CREATE INDEX journal_retirements_storage
    ON journal_retirements(storage_id, delete_after, object_key);
