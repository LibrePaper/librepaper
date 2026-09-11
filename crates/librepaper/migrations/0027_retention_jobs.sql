-- Durable account preference applications.  A generation is the idempotency
-- key for both the HTTP request and the bounded background thinning pass.
CREATE TABLE quota_retention_jobs (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    generation TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    payload TEXT NOT NULL CHECK (length(CAST(payload AS BLOB)) <= 65536),
    candidate_fingerprint TEXT NOT NULL,
    -- -1 means this job has no deployment hard-count override.  A nonnegative
    -- value is persisted so the worker can enforce the cap after the grace
    -- window, even though the live server configuration may have changed.
    hard_count_limit INTEGER NOT NULL DEFAULT -1 CHECK (hard_count_limit >= -1),
    -- Pressure jobs have a distinct eviction policy; this is not encoded by
    -- pretending the deployment count limit is zero.
    pressure INTEGER NOT NULL DEFAULT 0 CHECK (pressure IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('pending', 'grace', 'running', 'complete', 'stale')),
    grace_until INTEGER NOT NULL CHECK (grace_until >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    completed_at INTEGER,
    PRIMARY KEY (account_id, generation)
) WITHOUT ROWID;
CREATE INDEX quota_retention_jobs_due ON quota_retention_jobs(status, grace_until);

-- Candidate rows are separate from the preference record so a retry can be
-- resumed after a process crash without recomputing a different candidate
-- set.  The row remains charged until its grace period and object cleanup
-- have completed.
CREATE TABLE quota_retention_candidates (
    account_id TEXT NOT NULL,
    generation TEXT NOT NULL,
    slug TEXT NOT NULL,
    sha TEXT NOT NULL,
    planned_seq INTEGER NOT NULL DEFAULT -1,
    planned_parent TEXT NOT NULL DEFAULT '',
    planned_tree_sha TEXT NOT NULL DEFAULT '',
    planned_at TEXT NOT NULL DEFAULT '',
    planned_label TEXT NOT NULL DEFAULT '',
    planned_why TEXT NOT NULL DEFAULT '',
    grace_until INTEGER NOT NULL CHECK (grace_until >= 0),
    pressure_class INTEGER NOT NULL DEFAULT 0 CHECK (pressure_class IN (0, 1)),
    pressure_ordinal INTEGER NOT NULL DEFAULT 0 CHECK (pressure_ordinal >= 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'removed', 'blocked')),
    PRIMARY KEY (account_id, generation, slug, sha),
    FOREIGN KEY (account_id, generation)
        REFERENCES quota_retention_jobs(account_id, generation) ON DELETE CASCADE,
    FOREIGN KEY (slug) REFERENCES documents(slug) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX quota_retention_candidates_due
    ON quota_retention_candidates(status, grace_until, slug, sha);

-- Retention metadata is kept outside the immutable manifest payload.  This
-- lets catalogue-only pruning record a gap without rewriting source trees.
CREATE TABLE checkpoint_retention (
    slug TEXT NOT NULL,
    sha TEXT NOT NULL,
    original_parent TEXT NOT NULL DEFAULT '',
    ancestry_gap INTEGER NOT NULL DEFAULT 0 CHECK (ancestry_gap IN (0, 1)),
    grace_until INTEGER NOT NULL DEFAULT 0 CHECK (grace_until >= 0),
    lease_until INTEGER NOT NULL DEFAULT 0 CHECK (lease_until >= 0),
    PRIMARY KEY (slug, sha),
    FOREIGN KEY (slug, sha) REFERENCES checkpoints(slug, sha) ON DELETE CASCADE
) WITHOUT ROWID;

-- Existing documents retain their historical policy until an owner confirms
-- a migration. New documents are enrolled in Balanced at creation time.
CREATE TABLE document_retention_policy (
    slug TEXT PRIMARY KEY REFERENCES documents(slug) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK (mode IN ('legacy', 'balanced', 'custom')),
    policy_version INTEGER NOT NULL DEFAULT 1 CHECK (policy_version >= 1),
    enrolled_at INTEGER NOT NULL CHECK (enrolled_at >= 0),
    last_scheduled_at INTEGER NOT NULL DEFAULT 0 CHECK (last_scheduled_at >= 0)
) WITHOUT ROWID;
INSERT INTO document_retention_policy(slug,mode,policy_version,enrolled_at)
    SELECT slug,'legacy',1,0 FROM documents;

-- Rendering timestamps are publication metadata, not a durable ordering key:
-- PDF and SyncTeX arrive as separate uploads and may share a timestamp.  The
-- ordinal is assigned once, when the PDF first becomes registered, and is
-- retained when the companion SyncTeX/provenance metadata arrives later.
ALTER TABLE renderings ADD COLUMN published_seq INTEGER NOT NULL DEFAULT 0
    CHECK (published_seq >= 0);
UPDATE renderings AS current
SET published_seq = (
    SELECT COUNT(*)
    FROM renderings AS prior
    WHERE prior.bytes > 0
      AND prior.slug = current.slug
      AND (prior.at < current.at
           OR (prior.at = current.at AND prior.tree_sha <= current.tree_sha))
)
WHERE current.bytes > 0;
CREATE INDEX renderings_publication_order
    ON renderings(slug, published_seq DESC, at DESC, tree_sha DESC);
