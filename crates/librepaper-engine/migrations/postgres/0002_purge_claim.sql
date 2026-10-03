-- Claim physical deletion before touching blobs, so restoration cannot race a
-- purge that has already started.
ALTER TABLE documents DROP CONSTRAINT documents_status_check;
ALTER TABLE documents DROP CONSTRAINT documents_check;
ALTER TABLE documents
    ADD CONSTRAINT documents_status_check
    CHECK (status IN ('active', 'deleting', 'purging'));
ALTER TABLE documents
    ADD CONSTRAINT documents_check
    CHECK ((status IN ('deleting', 'purging')) = (deleted_at IS NOT NULL));

DROP INDEX documents_deleting;
CREATE INDEX documents_deleting ON documents(id)
    WHERE status IN ('deleting', 'purging');
