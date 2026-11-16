-- Shared results: one Quarto output record per project, uploaded by an editor
-- after a local render. It is not a project file. Its body bytes count toward
-- the document owner's storage through usage_bytes (Sources::RESULTS), so no
-- trigger and no counter is involved. Deleting the project deletes the record.
CREATE TABLE document_results (
    document_id uuid PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    body text NOT NULL,
    byte_length bigint NOT NULL CHECK (byte_length >= 0),
    uploaded_by uuid REFERENCES accounts(id) ON DELETE SET NULL,
    uploader_name text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
