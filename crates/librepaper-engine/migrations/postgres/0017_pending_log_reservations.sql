-- Quota reserved for source batches accepted into a live sequencer but not
-- yet committed to document_updates. The process writer epoch makes crash
-- recovery safe after the deployment advisory lease has been claimed.
CREATE TABLE pending_log_reservations (
    reservation_id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    writer_epoch bigint NOT NULL,
    bytes bigint NOT NULL CHECK (bytes > 0),
    batch_digest bytea NOT NULL CHECK (octet_length(batch_digest) = 32)
);

CREATE INDEX pending_log_reservations_document_idx
    ON pending_log_reservations(document_id, writer_epoch);
