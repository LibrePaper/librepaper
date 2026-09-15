-- One archive, however many versions name it.
--
-- A version's archive is named by the digest of its own bytes, so two
-- versions holding the same document name the same object and it is stored
-- once. That makes `archive_key` a reference rather than a possession, and
-- the uniqueness constraint that treated it as a possession has to go.
ALTER TABLE document_versions DROP CONSTRAINT document_versions_archive_key_key;

-- Dropping the constraint drops the index it was enforced with, and the
-- storage accounting below looks up siblings by key on every version write
-- and delete.
CREATE INDEX document_versions_archive_key ON document_versions(archive_key);

-- Deployment storage counts objects, not references.
--
-- The counter this trigger maintains is the number of bytes actually held in
-- the blob store. When a version shares an archive that is already there, it
-- adds no bytes, and charging for them would bill a deployment for storage it
-- never used. The mirror case matters more: refunding on every delete would
-- decrement the counter for bytes that other versions still reference, and a
-- counter that drifts below the truth eventually admits writes that the
-- deployment has no room for.
--
-- So each side asks the same question -- is this row the only one naming this
-- object? -- and moves the counter only then. `id IS DISTINCT FROM` is what
-- makes it correct in an AFTER trigger for both operations: on INSERT the new
-- row is already visible and must not count itself, and on DELETE the old row
-- is already gone.
CREATE OR REPLACE FUNCTION adjust_version_storage_usage() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    shared boolean;
BEGIN
    IF TG_OP = 'INSERT' THEN
        SELECT EXISTS(SELECT 1 FROM document_versions
                      WHERE archive_key = NEW.archive_key AND id IS DISTINCT FROM NEW.id)
          INTO shared;
        IF NOT shared THEN
            UPDATE storage_usage SET bytes = bytes + NEW.archive_bytes;
        END IF;
        RETURN NEW;
    ELSE
        SELECT EXISTS(SELECT 1 FROM document_versions
                      WHERE archive_key = OLD.archive_key AND id IS DISTINCT FROM OLD.id)
          INTO shared;
        IF NOT shared THEN
            UPDATE storage_usage SET bytes = bytes - OLD.archive_bytes;
        END IF;
        RETURN OLD;
    END IF;
END
$$;

-- The counter was accumulated under the old rule, which charged once per
-- version. Rebuild it under the new one so that the first write after this
-- migration is measured against the truth rather than against the drift.
UPDATE storage_usage SET bytes = (
    SELECT COALESCE(sum(bytes), 0)::bigint FROM (
        SELECT byte_length AS bytes FROM document_assets
        UNION ALL SELECT archive_bytes FROM (
            SELECT DISTINCT ON (archive_key) archive_bytes
            FROM document_versions ORDER BY archive_key, id
        ) distinct_archives
        UNION ALL SELECT byte_length FROM publication_files
    ) held
);
