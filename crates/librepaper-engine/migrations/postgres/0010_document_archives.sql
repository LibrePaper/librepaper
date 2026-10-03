-- Archive bytes belong to the object, not to every label that names it.
-- Freeze both accounting inputs while converting the denormalized rows.
LOCK TABLE document_assets, document_labels IN ACCESS EXCLUSIVE MODE;
LOCK TABLE storage_usage IN ACCESS EXCLUSIVE MODE;

CREATE TABLE document_archives (
    storage_key text PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- Legacy archive rows predate content digests and may have no tree digest.
    -- New objects populate both values; NULL is retained only for old data.
    tree_digest bytea CHECK (tree_digest IS NULL OR octet_length(tree_digest) = 32),
    content_digest bytea CHECK (content_digest IS NULL OR octet_length(content_digest) = 32),
    byte_length bigint NOT NULL CHECK (byte_length >= 0),
    UNIQUE (document_id, storage_key)
);

-- A key has always denoted one immutable object. Fail clearly if historical
-- rows assigned that same key to multiple documents or conflicting sizes.
DO $$
BEGIN
    IF EXISTS (
        SELECT archive_key FROM document_labels
        WHERE archive_key IS NOT NULL
        GROUP BY archive_key
        HAVING count(DISTINCT document_id) > 1
            OR min(archive_bytes) IS DISTINCT FROM max(archive_bytes)
            OR min(archive_bytes) IS NULL
    ) THEN
        RAISE EXCEPTION 'legacy archive keys have conflicting document or size metadata';
    END IF;
END
$$;

-- Existing keys and sizes are authoritative. Keep legacy NULL digests NULL;
-- the migration must not invent a digest for bytes it cannot inspect.
INSERT INTO document_archives (storage_key, document_id, tree_digest, byte_length)
SELECT archive_key, document_id,
       CASE WHEN count(DISTINCT tree_digest) = 1
            THEN (array_agg(tree_digest) FILTER (WHERE tree_digest IS NOT NULL))[1]
       END,
       max(archive_bytes)
FROM document_labels
WHERE archive_key IS NOT NULL
GROUP BY archive_key, document_id;

ALTER TABLE document_labels
    ADD CONSTRAINT document_labels_archive_fk
    FOREIGN KEY (document_id, archive_key)
    REFERENCES document_archives (document_id, storage_key)
    ON DELETE CASCADE;

-- The key is the archive relationship. Weight and digests live once on the
-- object row, so labels can no longer disagree about object metadata.
DROP TRIGGER IF EXISTS document_labels_storage_usage_insert ON document_labels;
DROP TRIGGER IF EXISTS document_labels_storage_usage_update ON document_labels;
DROP TRIGGER IF EXISTS document_labels_storage_usage_delete ON document_labels;
DROP FUNCTION IF EXISTS adjust_label_archive_storage_insert();
DROP FUNCTION IF EXISTS adjust_label_archive_storage_update();
DROP FUNCTION IF EXISTS adjust_label_archive_storage_delete();

ALTER TABLE document_labels DROP COLUMN archive_bytes;

CREATE FUNCTION adjust_document_archive_storage() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    delta bigint;
BEGIN
    IF TG_OP = 'INSERT' THEN
        delta := NEW.byte_length;
    ELSIF TG_OP = 'DELETE' THEN
        delta := -OLD.byte_length;
    ELSE
        delta := NEW.byte_length - OLD.byte_length;
    END IF;
    IF delta <> 0 THEN
        UPDATE storage_usage SET bytes = bytes + delta WHERE singleton;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'storage_usage singleton is missing';
        END IF;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER document_archives_storage_usage
    AFTER INSERT OR UPDATE OR DELETE ON document_archives
    FOR EACH ROW EXECUTE FUNCTION adjust_document_archive_storage();

-- Reconcile the counter against the normalized source of truth while its
-- inputs remain locked. Assets are unchanged; archive bytes now count once
-- per retained object, including an object with no remaining label.
SELECT singleton FROM storage_usage WHERE singleton FOR UPDATE;
UPDATE storage_usage
SET bytes = (SELECT COALESCE(sum(byte_length), 0)::bigint FROM document_assets)
          + (SELECT COALESCE(sum(byte_length), 0)::bigint FROM document_archives)
WHERE singleton;
