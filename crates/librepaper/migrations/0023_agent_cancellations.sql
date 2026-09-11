-- A cancellation is an authority decision, not an in-memory task flag.
-- Keep it separate from catalog_operations: a candidate may not have reached
-- its final annotation/source operation when the caller cancels it.
CREATE TABLE agent_cancellations (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    target_request_id TEXT NOT NULL,
    cancel_request_id TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    status TEXT NOT NULL,
    result TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(storage_id, cancel_request_id),
    UNIQUE(storage_id, cancel_request_id)
);
CREATE INDEX agent_cancellations_expiry ON agent_cancellations(created_at);
CREATE INDEX agent_cancellations_target ON agent_cancellations(storage_id, target_request_id);
