-- Cover every branch of the authorization-aware listing query.  The query
-- merges these already ordered pages in Rust, so SQLite never has to sort or
-- de-duplicate a large OR/EXISTS result into a temporary B-tree.
CREATE INDEX IF NOT EXISTS documents_active_owner_updated
    ON documents(owner_id, updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL;
CREATE INDEX IF NOT EXISTS documents_active_owner_key_updated
    ON documents(owner_key, updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL AND owner_id IS NULL;
CREATE INDEX IF NOT EXISTS documents_active_example_updated
    ON documents(updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL AND example = 1;
-- Visibility joins (grants and guests) still need to emit documents in the
-- same order as the owner/example branches.  The query deliberately scans
-- this narrow partial index and probes the account-side covering indexes, so
-- SQLite never builds a temporary sort for a bounded page.
CREATE INDEX IF NOT EXISTS documents_active_updated
    ON documents(updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL;
CREATE INDEX IF NOT EXISTS grants_account_slug
    ON grants(account_id, slug);
CREATE INDEX IF NOT EXISTS guests_account_slug
    ON guests(account_id, slug, link_hash);
CREATE INDEX IF NOT EXISTS links_slug_hash_until
    ON links(slug, hash, until);
