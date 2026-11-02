-- Who wrote each Loro peer of a document. A browser session gets a random
-- PeerID, so Loro alone cannot say which account typed a character; this table
-- records the mapping once, when a session first writes. The first writer wins,
-- so a peer never changes hands after it has been attributed.
CREATE TABLE document_peers (
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- Loro PeerID (u64) stored bit-for-bit as bigint.
    peer bigint NOT NULL,
    author_account_id uuid REFERENCES accounts(id),
    -- SHA-256 of the agent token that wrote it, for server-side agent edits.
    link_hash bytea CHECK (link_hash IS NULL OR octet_length(link_hash) = 32),
    author_label text NOT NULL DEFAULT '',
    first_seen timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (document_id, peer)
);
