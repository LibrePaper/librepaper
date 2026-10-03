-- A signed-in visitor to a share link is recorded as a grant for listing and
-- attribution purposes, but the link remains the source of its authority.
-- Keeping a copied role here let later link edits leave a stronger stale
-- grant behind.
ALTER TABLE grants ALTER COLUMN role DROP NOT NULL;

UPDATE grants SET role = NULL WHERE source_link_hash IS NOT NULL;

ALTER TABLE grants
    ADD CONSTRAINT grants_role_provenance CHECK (
        (source_link_hash IS NULL) = (role IS NOT NULL)
    );
