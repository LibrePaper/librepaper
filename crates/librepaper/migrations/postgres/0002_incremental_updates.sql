-- Activity measures the full history independently of the incremental payload.
-- Existing full-history rows retain their original measurement through COALESCE.
ALTER TABLE document_updates ADD COLUMN state_bytes bigint CHECK (state_bytes >= 0);
