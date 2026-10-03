-- Compact MCP outcomes commit alongside the mutation they describe. These are
-- actor-scoped recovery evidence, never permission to execute a write again.
CREATE TABLE operation_outcomes (
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    actor text NOT NULL CHECK (octet_length(actor) BETWEEN 1 AND 512),
    request_id text NOT NULL CHECK (octet_length(request_id) BETWEEN 1 AND 128),
    digest text NOT NULL CHECK (octet_length(digest) = 64),
    tool text NOT NULL CHECK (tool IN ('document_propose', 'document_apply', 'document_comment')),
    outcome jsonb NOT NULL CHECK (octet_length(outcome::text) <= 32768),
    expires_at bigint NOT NULL,
    PRIMARY KEY (document_id, actor, request_id)
);
CREATE INDEX operation_outcomes_expiry ON operation_outcomes(expires_at);
