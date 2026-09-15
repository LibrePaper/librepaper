-- Keep hard collaboration admission O(1) at the document row lock.
ALTER TABLE documents
    ADD COLUMN uncompacted_update_count bigint NOT NULL DEFAULT 0
        CHECK (uncompacted_update_count >= 0),
    ADD COLUMN uncompacted_update_bytes bigint NOT NULL DEFAULT 0
        CHECK (uncompacted_update_bytes >= 0);

UPDATE documents d SET
    uncompacted_update_count = u.update_count,
    uncompacted_update_bytes = u.update_bytes
FROM (
    SELECT document_id, count(*)::bigint update_count,
           COALESCE(sum(octet_length(update_bytes)), 0)::bigint update_bytes
    FROM document_updates GROUP BY document_id
) u
WHERE d.id = u.document_id;
