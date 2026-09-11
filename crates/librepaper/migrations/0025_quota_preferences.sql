-- Versioned account intent.  The payload is kept as JSON so a newer server
-- can preserve fields it does not understand instead of silently replacing a
-- preference with an invented default.
CREATE TABLE account_quota_preferences (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    payload TEXT NOT NULL CHECK (length(CAST(payload AS BLOB)) <= 65536),
    policy_generation TEXT NOT NULL,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
) WITHOUT ROWID;
