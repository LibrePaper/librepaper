-- Durable Quarto selection pointers.  The blob store may contain a staged or
-- rolled-back selection object, but readers must follow this catalogue row.
-- Keeping the pointer in the existing catalogue makes selection publication
-- recoverable after a process restart and leaves the previous selection intact
-- when a fenced object commit is rejected.
CREATE TABLE quarto_selections (
    storage_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    render_id TEXT NOT NULL,
    source_revision TEXT NOT NULL,
    object_key TEXT NOT NULL,
    object_version TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (storage_id, document_id, context_id),
    FOREIGN KEY (storage_id) REFERENCES documents(storage_id) ON DELETE CASCADE
);
CREATE INDEX quarto_selections_by_storage
    ON quarto_selections (storage_id, document_id, context_id);
