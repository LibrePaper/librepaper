-- Immutable agent objects are scoped to both a document and authenticated actor.
-- Document deletion also removes these retained source views and candidates.
CREATE TABLE agent_objects (
    slug TEXT NOT NULL REFERENCES documents(slug) ON DELETE CASCADE,
    actor TEXT NOT NULL,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY(slug, actor, id, kind)
);
CREATE INDEX agent_objects_expiry ON agent_objects(expires_at);
CREATE INDEX agent_receipts_expiry ON catalog_operations(created_at)
    WHERE kind IN ('agent_apply','agent_annotations','agent_cancel') AND status <> 'prepared';
