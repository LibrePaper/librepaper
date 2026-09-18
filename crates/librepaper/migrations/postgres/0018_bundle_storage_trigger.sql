-- Migration 17 renamed the tables and columns, but PostgreSQL does not rewrite
-- SQL inside PL/pgSQL function bodies. Keep the existing trigger attachment
-- and function name while updating the references used on insert and delete.
CREATE OR REPLACE FUNCTION adjust_publication_file_storage_usage() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    shared boolean;
BEGIN
    IF TG_OP = 'INSERT' THEN
        SELECT EXISTS(SELECT 1 FROM document_assets WHERE storage_key = NEW.storage_key)
            OR EXISTS(SELECT 1 FROM bundle_files
                      WHERE storage_key = NEW.storage_key
                        AND (bundle_id, path) IS DISTINCT FROM (NEW.bundle_id, NEW.path))
          INTO shared;
        IF NOT shared THEN
            UPDATE storage_usage SET bytes = bytes + NEW.byte_length;
        END IF;
        RETURN NEW;
    ELSE
        SELECT EXISTS(SELECT 1 FROM document_assets WHERE storage_key = OLD.storage_key)
            OR EXISTS(SELECT 1 FROM bundle_files
                      WHERE storage_key = OLD.storage_key
                        AND (bundle_id, path) IS DISTINCT FROM (OLD.bundle_id, OLD.path))
          INTO shared;
        IF NOT shared THEN
            UPDATE storage_usage SET bytes = bytes - OLD.byte_length;
        END IF;
        RETURN OLD;
    END IF;
END
$$;
