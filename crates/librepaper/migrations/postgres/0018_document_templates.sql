-- A custom template is an ordinary project plus a row here. The row is the
-- whole of the mark: the project keeps its owner, its quota charge and its
-- trash lifecycle, and the cascade drops the mark with the document.
CREATE TABLE document_templates (
    document_id uuid PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);
