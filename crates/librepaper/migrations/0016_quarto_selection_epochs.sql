-- A cleared selection still consumes a generation.  Keeping the epoch
-- separately from the selected render prevents an in-flight request created
-- before a source restore from publishing with the old expected_generation.
CREATE TABLE quarto_selection_epochs (
    storage_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (storage_id, document_id, context_id),
    FOREIGN KEY (storage_id) REFERENCES documents(storage_id) ON DELETE CASCADE
);
