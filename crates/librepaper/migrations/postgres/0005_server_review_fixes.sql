-- Persist metadata which is part of authorization and rate-limit decisions.
ALTER TABLE share_links ADD COLUMN comment_budget bigint
    CHECK (comment_budget IS NULL OR comment_budget >= 0);

ALTER TABLE grants ADD COLUMN source_link_hash bytea
    CHECK (source_link_hash IS NULL OR octet_length(source_link_hash) = 32);

-- One durable deployment-scoped payload. The server owns its JSON schema and
-- replaces the value atomically at each checkpoint.
CREATE TABLE server_runtime_state (
    name text PRIMARY KEY,
    value jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
