-- Org-mode documents: the source_format check gains 'org'. The initial catalog
-- named the formats inline, and Postgres names that check after its table and
-- column, so it is replaced rather than edited in place.
ALTER TABLE documents DROP CONSTRAINT documents_source_format_check;
ALTER TABLE documents ADD CONSTRAINT documents_source_format_check
    CHECK (source_format IN ('markdown','html','typst','latex','quarto','org'));
