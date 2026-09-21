-- Tighten catalogue invariants and keep only indexes used by current access paths.
--
-- These checks are intentionally validated when the migration runs. If a
-- deployment contains an old malformed row, migration must stop and expose it
-- for repair rather than silently converting it into a fabricated annotation.

ALTER TABLE annotations
    DROP CONSTRAINT annotations_source_text_target;

ALTER TABLE annotations
    ADD CONSTRAINT annotations_source_text_target CHECK (
        (target_kind = 'source_text'
            AND file_id IS NOT NULL
            AND start_utf16 IS NOT NULL AND start_utf16 >= 0
            AND end_utf16 IS NOT NULL AND end_utf16 >= start_utf16
            AND start_side IS NOT NULL AND start_side IN ('left', 'right')
            AND end_side IS NOT NULL AND end_side IN ('left', 'right')
            AND exact IS NOT NULL AND prefix IS NOT NULL AND suffix IS NOT NULL)
        OR
        (target_kind = 'document'
            AND file_id IS NULL
            AND start_utf16 IS NULL AND end_utf16 IS NULL
            AND start_side IS NULL AND end_side IS NULL
            AND exact IS NULL AND prefix IS NULL AND suffix IS NULL)
    );

ALTER TABLE annotations
    ADD CONSTRAINT annotations_evidence_required
    CHECK (source_sequence IS NOT NULL AND frontier IS NOT NULL);

DROP INDEX document_labels_timeline;
DROP INDEX document_labels_archive_pending;
CREATE INDEX document_labels_archive_pending
    ON document_labels(id)
    WHERE archive_requested_at IS NOT NULL AND archive_key IS NULL;

DROP INDEX document_marks_favorites;
DROP INDEX document_marks_recent;
DROP INDEX annotations_source_file;

CREATE INDEX document_marks_document ON document_marks(document_id);

CREATE INDEX documents_examples_updated ON documents(updated_at DESC, id DESC)
    WHERE status = 'active' AND ownership_mode = 'example';

DROP INDEX share_links_document;
CREATE INDEX share_links_document ON share_links(document_id, role);

CREATE INDEX annotations_author_account
    ON annotations(author_account_id)
    WHERE author_account_id IS NOT NULL;
CREATE INDEX replies_author_account
    ON replies(author_account_id)
    WHERE author_account_id IS NOT NULL;
CREATE INDEX document_labels_author_account
    ON document_labels(author_account_id)
    WHERE author_account_id IS NOT NULL;
