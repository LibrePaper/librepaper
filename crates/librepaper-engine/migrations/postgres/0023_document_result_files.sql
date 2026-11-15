-- Binary files of a shared results record (run figures, Calepin images),
-- stored as raw bytes instead of base64 inside the record body. The record's
-- document_results.byte_length charges the body text plus these raw bytes, so
-- usage_bytes (Sources::RESULTS) needs no second term. Replaced whole with the
-- record; deleting the project deletes the files.
CREATE TABLE document_result_files (
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    path text NOT NULL,
    byte_length bigint NOT NULL CHECK (byte_length >= 0),
    bytes bytea NOT NULL,
    PRIMARY KEY (document_id, path)
);
