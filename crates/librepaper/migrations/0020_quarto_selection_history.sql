-- Only successful selections establish eligibility for automatic restore.
-- Old bundles alone cannot establish that they were ever selected.
CREATE TABLE quarto_selection_history (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    document_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    render_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (storage_id, document_id, context_id, render_id)
) WITHOUT ROWID;
INSERT INTO quarto_selection_history
    SELECT storage_id, document_id, context_id, render_id, generation FROM quarto_selections;
