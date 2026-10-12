-- Names refused before their first sign-in. The first sign-in whose provider
-- and handle match a row creates its account already blocked and deletes the
-- row, so from then on the block follows the provider's stable subject and
-- not a name the person can change.
CREATE TABLE banned_handles (
    id uuid PRIMARY KEY,
    provider text NOT NULL CHECK (provider IN ('github', 'google')),
    handle text NOT NULL CHECK (length(btrim(handle)) > 0),
    actor text NOT NULL CHECK (length(btrim(actor)) > 0),
    reason text NOT NULL CHECK (length(btrim(reason)) > 0),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX banned_handles_name ON banned_handles(provider, lower(handle));

ALTER TABLE moderation_audit DROP CONSTRAINT moderation_audit_action_check;
ALTER TABLE moderation_audit ADD CONSTRAINT moderation_audit_action_check
    CHECK (action IN ('block_account','unblock_account','hide_project','unhide_project','ban_handle','unban_handle'));
ALTER TABLE moderation_audit DROP CONSTRAINT moderation_audit_target_kind_check;
ALTER TABLE moderation_audit ADD CONSTRAINT moderation_audit_target_kind_check
    CHECK (target_kind IN ('account','project','handle'));
