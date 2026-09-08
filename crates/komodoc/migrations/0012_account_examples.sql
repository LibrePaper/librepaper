-- Keep completion records after a starter document is deleted, so signing in
-- never recreates it. Existing accounts are not enrolled by this migration.
CREATE TABLE account_examples (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 3),
    slug TEXT NOT NULL UNIQUE,
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    PRIMARY KEY (account_id, position)
) WITHOUT ROWID;
