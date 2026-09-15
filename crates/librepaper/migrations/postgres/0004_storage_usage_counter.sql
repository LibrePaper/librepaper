CREATE TABLE storage_usage (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    bytes bigint NOT NULL CHECK (bytes >= 0)
);

INSERT INTO storage_usage(bytes)
SELECT COALESCE(sum(bytes), 0)::bigint FROM (
    SELECT byte_length AS bytes FROM document_assets
    UNION ALL SELECT archive_bytes FROM document_versions
    UNION ALL SELECT byte_length FROM publication_files
) existing;

CREATE FUNCTION adjust_asset_storage_usage() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE storage_usage SET bytes = bytes + CASE WHEN TG_OP='INSERT' THEN NEW.byte_length ELSE -OLD.byte_length END;
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END
$$;
CREATE FUNCTION adjust_version_storage_usage() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE storage_usage SET bytes = bytes + CASE WHEN TG_OP='INSERT' THEN NEW.archive_bytes ELSE -OLD.archive_bytes END;
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END
$$;
CREATE FUNCTION adjust_publication_file_storage_usage() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE storage_usage SET bytes = bytes + CASE WHEN TG_OP='INSERT' THEN NEW.byte_length ELSE -OLD.byte_length END;
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END
$$;

CREATE TRIGGER document_assets_storage_usage AFTER INSERT OR DELETE ON document_assets
    FOR EACH ROW EXECUTE FUNCTION adjust_asset_storage_usage();
CREATE TRIGGER document_versions_storage_usage AFTER INSERT OR DELETE ON document_versions
    FOR EACH ROW EXECUTE FUNCTION adjust_version_storage_usage();
CREATE TRIGGER publication_files_storage_usage AFTER INSERT OR DELETE ON publication_files
    FOR EACH ROW EXECUTE FUNCTION adjust_publication_file_storage_usage();
