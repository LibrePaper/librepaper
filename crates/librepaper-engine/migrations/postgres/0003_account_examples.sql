-- When each account received its starter documents. Null means the account
-- has not been provisioned yet, and its next sign-in provisions it. The
-- backfill marks every existing account as provisioned, except those whose
-- old 8-character starter slug already belongs to another account. Those
-- accounts receive their starters on their next sign-in.
ALTER TABLE accounts ADD COLUMN examples_at timestamptz;
UPDATE accounts a SET examples_at = a.created_at
 WHERE NOT EXISTS (
     SELECT 1 FROM documents d
      WHERE d.slug = 'starter-1-' || left(a.id::text, 8)
        AND d.owner_id <> a.id);
