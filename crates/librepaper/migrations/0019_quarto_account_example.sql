-- Preserve completed and pending slots for existing accounts. Only new
-- accounts are enrolled in the additional Quarto starter by application code.
CREATE TABLE account_examples_next (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 4),
    slug TEXT NOT NULL UNIQUE,
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    PRIMARY KEY (account_id, position)
) WITHOUT ROWID;
INSERT INTO account_examples_next SELECT account_id, position, slug, completed FROM account_examples;
DROP TABLE account_examples;
ALTER TABLE account_examples_next RENAME TO account_examples;
