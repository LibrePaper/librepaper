-- Reversible operator controls. A hidden document remains active and keeps
-- every source, asset, history row, and share grant; public access is denied
-- by the application while this row exists.
CREATE TABLE moderated_projects (
    document_id uuid PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    hidden_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE moderation_audit (
    id uuid PRIMARY KEY,
    actor text NOT NULL CHECK (length(btrim(actor)) > 0),
    occurred_at timestamptz NOT NULL DEFAULT now(),
    action text NOT NULL CHECK (action IN ('block_account','unblock_account','hide_project','unhide_project')),
    target_kind text NOT NULL CHECK (target_kind IN ('account','project')),
    target_id uuid NOT NULL,
    target_label text NOT NULL,
    reason text NOT NULL CHECK (length(btrim(reason)) > 0)
);

CREATE INDEX moderation_audit_time ON moderation_audit(occurred_at DESC, id DESC);
