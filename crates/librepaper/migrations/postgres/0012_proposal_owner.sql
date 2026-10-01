-- Proposal authorship controls who may replace a pending branch. Store the
-- authenticated principal separately from the display label. Historical rows
-- have no trustworthy identity, so leave them NULL: they remain reviewable
-- and any editor can discard them, but nobody can claim ownership to update.
ALTER TABLE document_proposals
    DROP COLUMN author_peer,
    ADD COLUMN owner_key text;
