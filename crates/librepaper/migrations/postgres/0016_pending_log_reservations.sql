-- Quota reserved for source batches accepted into a live sequencer but not
-- yet committed to document_updates. The process writer epoch makes crash
-- recovery safe after the deployment advisory lease has been claimed.
CREATE TABLE pending_log_reservations (
    document_id uuid PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    writer_epoch bigint NOT NULL,
    bytes bigint NOT NULL CHECK (bytes > 0)
);
