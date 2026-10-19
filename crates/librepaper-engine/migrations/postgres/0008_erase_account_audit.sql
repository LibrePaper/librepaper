-- An operator can start erasing an account from the CLI, and the audit trail
-- records that as its own action.
ALTER TABLE moderation_audit DROP CONSTRAINT moderation_audit_action_check;
ALTER TABLE moderation_audit ADD CONSTRAINT moderation_audit_action_check
    CHECK (action IN ('block_account','unblock_account','hide_project','unhide_project','ban_handle','unban_handle','erase_account'));
