-- Provenance links must not prevent bounded version retention.
ALTER TABLE document_versions DROP CONSTRAINT document_versions_parent_id_fkey;
ALTER TABLE document_versions ADD CONSTRAINT document_versions_parent_id_fkey
    FOREIGN KEY (parent_id) REFERENCES document_versions(id) ON DELETE SET NULL;

ALTER TABLE publications DROP CONSTRAINT publications_source_version_id_fkey;
ALTER TABLE publications ADD CONSTRAINT publications_source_version_id_fkey
    FOREIGN KEY (source_version_id) REFERENCES document_versions(id) ON DELETE SET NULL;
