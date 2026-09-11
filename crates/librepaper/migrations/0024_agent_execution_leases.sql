-- A sidebar runner owns one live execution epoch per private conversation.
-- Replacing the runner changes the opaque epoch, fencing old in-flight work.
CREATE TABLE agent_execution_leases (
    slug TEXT NOT NULL REFERENCES documents(slug) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL,
    execution_epoch TEXT NOT NULL,
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    PRIMARY KEY (slug, conversation_id)
);
CREATE UNIQUE INDEX agent_execution_leases_epoch
    ON agent_execution_leases(slug, conversation_id, execution_epoch);
