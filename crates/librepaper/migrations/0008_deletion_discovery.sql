-- Deletion discovery is itself a resumable operation. A document may own
-- more than one bounded object-store page; keeping the prefix cursor in SQL
-- prevents a restart (or a max-1000 pass) from treating an incomplete listing
-- as a complete deletion.
CREATE TABLE deletion_discovery (
    slug TEXT NOT NULL REFERENCES documents(slug) ON DELETE CASCADE,
    prefix TEXT NOT NULL,
    cursor TEXT,
    done INTEGER NOT NULL DEFAULT 0 CHECK (done IN (0,1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (slug, prefix)
);
CREATE INDEX deletion_discovery_pending ON deletion_discovery(slug, done, prefix);
