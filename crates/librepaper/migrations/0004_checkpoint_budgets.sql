CREATE TABLE IF NOT EXISTS checkpoint_budgets (
    scope TEXT NOT NULL,
    bucket INTEGER NOT NULL CHECK (bucket >= 0),
    owner_key TEXT NOT NULL,
    used INTEGER NOT NULL CHECK (used >= 0),
    PRIMARY KEY (scope, bucket, owner_key)
) WITHOUT ROWID;
