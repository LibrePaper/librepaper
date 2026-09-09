-- Keep the historical source_format column as the compatibility wire value,
-- while giving readers and result publication an explicit engine/draft pair.
-- Rows created before this migration are normalized from that legacy value.
CREATE TABLE document_results_metadata (
    slug TEXT NOT NULL PRIMARY KEY REFERENCES documents(slug) ON DELETE CASCADE,
    execution_engine TEXT NOT NULL CHECK (execution_engine IN ('none', 'quarto')),
    draft_format TEXT NOT NULL CHECK (draft_format IN ('markdown', 'html', 'typst', 'latex', 'other'))
);

INSERT INTO document_results_metadata (slug, execution_engine, draft_format)
SELECT slug,
       CASE WHEN source_format = 'quarto' THEN 'quarto' ELSE 'none' END,
       CASE source_format
           WHEN 'quarto' THEN 'markdown'
           WHEN 'markdown' THEN 'markdown'
           WHEN '' THEN 'html'
           WHEN 'html' THEN 'html'
           WHEN 'typst' THEN 'typst'
           WHEN 'latex' THEN 'latex'
           ELSE 'other'
       END
FROM documents;

CREATE INDEX document_results_metadata_engine
    ON document_results_metadata(execution_engine, draft_format);

-- All existing insertion paths (including restores and seed documents) still
-- write the legacy documents row first.  Give each such row an explicit
-- default pair without changing those paths' SQL contracts.
CREATE TRIGGER document_results_metadata_after_insert
AFTER INSERT ON documents
BEGIN
    INSERT INTO document_results_metadata (slug, execution_engine, draft_format)
    VALUES (
        NEW.slug,
        CASE WHEN NEW.source_format = 'quarto' THEN 'quarto' ELSE 'none' END,
        CASE NEW.source_format
            WHEN 'quarto' THEN 'markdown'
            WHEN 'markdown' THEN 'markdown'
            WHEN '' THEN 'html'
            WHEN 'html' THEN 'html'
            WHEN 'typst' THEN 'typst'
            WHEN 'latex' THEN 'latex'
            ELSE 'other'
        END
    );
END;

CREATE TRIGGER document_results_metadata_after_source_format_update
AFTER UPDATE OF source_format ON documents
BEGIN
    UPDATE document_results_metadata
    SET execution_engine = CASE WHEN NEW.source_format = 'quarto' THEN 'quarto' ELSE 'none' END,
        draft_format = CASE NEW.source_format
            WHEN 'quarto' THEN 'markdown'
            WHEN 'markdown' THEN 'markdown'
            WHEN '' THEN 'html'
            WHEN 'html' THEN 'html'
            WHEN 'typst' THEN 'typst'
            WHEN 'latex' THEN 'latex'
            ELSE 'other'
        END
    WHERE slug = NEW.slug;
END;
