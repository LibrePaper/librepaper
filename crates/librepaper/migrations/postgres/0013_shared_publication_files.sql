-- A published page owns its rendering. It does not own the figures in it.
--
-- A publication used to be a self-contained copy: the rendered HTML, and
-- beside it a fresh copy of every figure, stylesheet and font the page named.
-- The figures were already here. `document_assets` is content-addressed and
-- unique per `(document_id, digest, byte_length)`, nothing in the codebase
-- deletes from it, and the orphan sweeper treats every key it names as live.
-- So the copy secured nothing that was not already secure, and the digest a
-- publication names will resolve to the same bytes for as long as the
-- document exists.
--
-- That was affordable while publishing was a button pressed a few times.
-- The reader version now rebuilds itself whenever the source goes quiet, so
-- the copy is made every few minutes, and a paper with figures fills a
-- 100 MB account in a day. What a publication actually has to keep is the
-- rendered page, because no server here can produce one: the renderers are
-- pinned WebAssembly that runs in the author's browser.
--
-- `storage_key` therefore becomes a reference rather than a possession, and
-- the uniqueness constraint that treated it as a possession has to go.
ALTER TABLE publication_files DROP CONSTRAINT publication_files_storage_key_key;

-- Dropping the constraint drops the index it was enforced with, and the
-- storage accounting below looks up siblings by key on every publication
-- file write and delete. The orphan sweeper reads this column too.
CREATE INDEX publication_files_storage_key ON publication_files(storage_key);

-- Deployment storage counts objects, not references.
--
-- Exactly the rule `document_versions` already follows for shared archives:
-- a row that names an object some other row also names adds no bytes, and
-- refunding on its delete would decrement the counter for bytes that are
-- still held. Both sides ask the same question -- is this row the only one
-- naming this object? -- and `id IS DISTINCT FROM` cannot be used here,
-- because a publication file has no id: its key is `(publication_id, path)`.
--
-- A figure shared with `document_assets` is never the only reference, so it
-- is counted there and not again here. That is the asymmetry this migration
-- exists to create, and it is why the recount at the end is not a no-op.
CREATE OR REPLACE FUNCTION adjust_publication_file_storage_usage() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    shared boolean;
BEGIN
    IF TG_OP = 'INSERT' THEN
        SELECT EXISTS(SELECT 1 FROM document_assets WHERE storage_key = NEW.storage_key)
            OR EXISTS(SELECT 1 FROM publication_files
                      WHERE storage_key = NEW.storage_key
                        AND (publication_id, path) IS DISTINCT FROM (NEW.publication_id, NEW.path))
          INTO shared;
        IF NOT shared THEN
            UPDATE storage_usage SET bytes = bytes + NEW.byte_length;
        END IF;
        RETURN NEW;
    ELSE
        SELECT EXISTS(SELECT 1 FROM document_assets WHERE storage_key = OLD.storage_key)
            OR EXISTS(SELECT 1 FROM publication_files
                      WHERE storage_key = OLD.storage_key
                        AND (publication_id, path) IS DISTINCT FROM (OLD.publication_id, OLD.path))
          INTO shared;
        IF NOT shared THEN
            UPDATE storage_usage SET bytes = bytes - OLD.byte_length;
        END IF;
        RETURN OLD;
    END IF;
END
$$;

-- The counter was accumulated under the old rule, which charged once per
-- publication file. Rebuild it under the new one so the first write after
-- this migration is measured against the truth rather than against the drift.
UPDATE storage_usage SET bytes = (
    SELECT COALESCE(sum(bytes), 0)::bigint FROM (
        SELECT byte_length AS bytes FROM document_assets
        UNION ALL SELECT archive_bytes FROM (
            SELECT DISTINCT ON (archive_key) archive_bytes
            FROM document_versions ORDER BY archive_key, id
        ) distinct_archives
        UNION ALL SELECT byte_length FROM (
            SELECT DISTINCT ON (f.storage_key) f.byte_length
            FROM publication_files f
            WHERE NOT EXISTS(SELECT 1 FROM document_assets a WHERE a.storage_key = f.storage_key)
            ORDER BY f.storage_key, f.publication_id, f.path
        ) distinct_publication_files
    ) held
);
