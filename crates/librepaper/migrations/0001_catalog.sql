CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    handle TEXT NOT NULL,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    plan TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'erasing', 'blocked')),
    session_generation TEXT NOT NULL,
    erasure_cursor TEXT CHECK (length(CAST(erasure_cursor AS BLOB)) <= 65536)
) WITHOUT ROWID;
CREATE TABLE documents (
    slug TEXT NOT NULL PRIMARY KEY,
    storage_id TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    sha TEXT NOT NULL,
    created_at TEXT NOT NULL,
    published_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    example INTEGER NOT NULL DEFAULT 0,
    owner_key TEXT NOT NULL,
    owner_id TEXT REFERENCES accounts (id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('creating', 'active', 'deleting')),
    size INTEGER NOT NULL CHECK (size >= 0),
    counted_size INTEGER NOT NULL CHECK (counted_size >= size),
    maintenance_reserved INTEGER NOT NULL DEFAULT 0
        CHECK (maintenance_reserved >= 0 AND maintenance_reserved <= counted_size),
    comment_seq INTEGER NOT NULL DEFAULT 0,
    last_auto_checkpoint_at INTEGER NOT NULL,
    pending_publication TEXT,
    last_publication_id TEXT NOT NULL DEFAULT '',
    source_format TEXT NOT NULL,
    main TEXT NOT NULL
);
CREATE INDEX documents_owner ON documents (owner_key) WHERE owner_id IS NULL;
CREATE INDEX documents_owner_id ON documents (owner_id);
CREATE INDEX documents_examples ON documents (slug) WHERE example = 1;
CREATE INDEX documents_expiry_created ON documents (created_at, slug)
    WHERE status = 'active' AND example = 0;
CREATE INDEX documents_expiry_updated ON documents (updated_at, slug)
    WHERE status = 'active' AND example = 0;
CREATE TABLE grants (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    role TEXT NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since TEXT NOT NULL,
    PRIMARY KEY (slug, role, account_id)
) WITHOUT ROWID;
CREATE INDEX grants_account ON grants (account_id);
CREATE TABLE links (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    role TEXT NOT NULL,
    hash TEXT NOT NULL,
    sealed BLOB NOT NULL,
    label TEXT NOT NULL,
    budget INTEGER,
    since TEXT NOT NULL,
    until TEXT NOT NULL, key_id TEXT NOT NULL DEFAULT 'legacy',
    PRIMARY KEY (slug, role)
) WITHOUT ROWID;
CREATE UNIQUE INDEX links_hash ON links (hash);
CREATE TABLE guests (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    since TEXT NOT NULL,
    link_hash TEXT NOT NULL,
    PRIMARY KEY (slug, account_id, link_hash)
) WITHOUT ROWID;
CREATE INDEX guests_account ON guests (account_id);
CREATE INDEX guests_link ON guests (slug, link_hash);
CREATE TABLE totals (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    documents INTEGER NOT NULL CHECK (documents >= 0)
);
INSERT INTO totals(id, bytes, documents) VALUES (1, 0, 0);
CREATE TABLE checkpoints (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    sha TEXT NOT NULL,
    seq INTEGER NOT NULL,
    durable_seq INTEGER NOT NULL CHECK (durable_seq >= 0),
    tree_sha TEXT NOT NULL,
    parent TEXT NOT NULL,
    at TEXT NOT NULL,
    by TEXT NOT NULL,
    why TEXT NOT NULL,
    source_format TEXT NOT NULL,
    size INTEGER NOT NULL,
    label TEXT NOT NULL,
    git_commit TEXT NOT NULL,
    dirty INTEGER NOT NULL,
    changed TEXT, by_account TEXT,
    PRIMARY KEY (slug, sha)
) WITHOUT ROWID;
CREATE UNIQUE INDEX checkpoints_sequence ON checkpoints (slug, seq);
CREATE TABLE comments (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    motivation TEXT NOT NULL,
    body TEXT NOT NULL,
    creator TEXT NOT NULL,
    author TEXT NOT NULL,
    via TEXT NOT NULL,
    created TEXT NOT NULL,
    exact TEXT NOT NULL,
    prefix TEXT NOT NULL,
    suffix TEXT NOT NULL,
    position INTEGER,
    region TEXT,
    source_path TEXT,
    source_exact TEXT,
    source_prefix TEXT,
    source_suffix TEXT,
    source_position INTEGER,
    proposed TEXT,
    outcome TEXT NOT NULL,
    accept_request TEXT NOT NULL,
    revision TEXT NOT NULL,
    resolved INTEGER NOT NULL DEFAULT 0,
    resolved_at TEXT,
    resolved_in TEXT NOT NULL, pass TEXT NOT NULL DEFAULT '', point INTEGER NOT NULL DEFAULT 0, color TEXT, quarto_output TEXT,
    PRIMARY KEY (slug, id)
);
CREATE UNIQUE INDEX comments_sequence ON comments (slug, seq);
CREATE TABLE replies (
    slug TEXT NOT NULL,
    comment_id TEXT NOT NULL,
    id TEXT NOT NULL,
    body TEXT NOT NULL,
    creator TEXT NOT NULL,
    author TEXT NOT NULL,
    created TEXT NOT NULL,
    PRIMARY KEY (slug, comment_id, id),
    FOREIGN KEY (slug, comment_id)
        REFERENCES comments (slug, id) ON DELETE CASCADE
);
CREATE TABLE renderings (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    tree_sha TEXT NOT NULL,
    at TEXT NOT NULL,
    backend TEXT NOT NULL,
    engine TEXT NOT NULL,
    release TEXT NOT NULL,
    tools TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    synctex INTEGER NOT NULL,
    synctex_bytes INTEGER NOT NULL, published_seq INTEGER NOT NULL DEFAULT 0
    CHECK (published_seq >= 0),
    PRIMARY KEY (slug, tree_sha)
) WITHOUT ROWID;
CREATE TABLE pending_deletes (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE RESTRICT,
    object_key TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    queued_at INTEGER NOT NULL,
    delete_after INTEGER NOT NULL,
    PRIMARY KEY (slug, object_key)
) WITHOUT ROWID;
CREATE INDEX pending_deletes_due ON pending_deletes (delete_after);
CREATE TABLE catalog_operations (
    storage_id TEXT NOT NULL REFERENCES documents (storage_id) ON DELETE CASCADE,
    request_id TEXT NOT NULL CHECK (length(CAST(request_id AS BLOB)) BETWEEN 1 AND 128),
    kind TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('prepared', 'committed', 'aborted')),
    intent TEXT NOT NULL CHECK (length(CAST(intent AS BLOB)) <= 65536),
    result TEXT NOT NULL CHECK (length(CAST(result AS BLOB)) <= 65536),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (storage_id, request_id)
);
CREATE INDEX catalog_operations_pending ON catalog_operations (storage_id, request_id)
    WHERE status = 'prepared';
CREATE TABLE journal_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    deployment_id TEXT NOT NULL,
    writer_generation TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    last_operation_id TEXT NOT NULL,
    next_segment_seq INTEGER NOT NULL CHECK (next_segment_seq >= 0),
    manifest_key TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    manifest_length INTEGER NOT NULL CHECK (manifest_length >= 0),
    tail_after INTEGER NOT NULL CHECK (tail_after >= 0)
);
INSERT INTO journal_state
    (id, deployment_id, writer_generation, revision, last_operation_id,
     next_segment_seq, manifest_key, manifest_digest, manifest_length, tail_after)
VALUES (1, '', '', 0, '', 0, '', '', 0, 0);
CREATE TABLE journal_preparations (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
    expected_generation TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    plan TEXT NOT NULL CHECK (length(CAST(plan AS BLOB)) <= 262144),
    resolved_at INTEGER
);
CREATE UNIQUE INDEX journal_preparations_unresolved
    ON journal_preparations (1) WHERE resolved_at IS NULL;
CREATE TABLE journal_segments (
    segment_id TEXT PRIMARY KEY,
    segment_seq INTEGER NOT NULL UNIQUE CHECK (segment_seq >= 0),
    operation_id TEXT NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    committed_at INTEGER NOT NULL
, storage_id TEXT NOT NULL DEFAULT '', epoch INTEGER NOT NULL DEFAULT 0 CHECK (epoch >= 0), first_sequence INTEGER NOT NULL DEFAULT 0 CHECK (first_sequence >= 0), last_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_sequence >= 0));
CREATE INDEX journal_segments_sequence ON journal_segments (segment_seq);
CREATE TABLE journal_retirements (
    object_key TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    payload_bytes INTEGER NOT NULL DEFAULT 0 CHECK (payload_bytes >= 0),
    maintenance_bytes INTEGER NOT NULL DEFAULT 0 CHECK (maintenance_bytes >= 0),
    retired_revision INTEGER,
    modified_at INTEGER NOT NULL,
    first_unreferenced_at INTEGER,
    delete_after INTEGER NOT NULL
, storage_id TEXT NOT NULL DEFAULT '');
CREATE INDEX journal_retirements_due
    ON journal_retirements (delete_after, object_key);
CREATE TABLE account_activity (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    last_qualified_at TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE erasure_batches (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    cursor TEXT,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE maintenance_jobs (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('active', 'released', 'failed')),
    reserved_bytes INTEGER NOT NULL CHECK (reserved_bytes >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX maintenance_jobs_status
    ON maintenance_jobs(status, updated_at, id);
CREATE INDEX checkpoints_cursor
    ON checkpoints(slug, seq, sha);
CREATE INDEX comments_cursor
    ON comments(slug, seq, id);
CREATE INDEX replies_cursor
    ON replies(slug, comment_id, created, id);
CREATE INDEX renderings_cursor
    ON renderings(slug, at, tree_sha);
CREATE INDEX documents_storage_status
    ON documents(storage_id, status);
CREATE TABLE checkpoint_budgets (
    scope TEXT NOT NULL,
    bucket INTEGER NOT NULL CHECK (bucket >= 0),
    owner_key TEXT NOT NULL,
    used INTEGER NOT NULL CHECK (used >= 0),
    PRIMARY KEY (scope, bucket, owner_key)
) WITHOUT ROWID;
CREATE INDEX journal_segments_storage_sequence
    ON journal_segments(storage_id, epoch, last_sequence);
CREATE TABLE journal_segment_coverage (
    segment_id TEXT NOT NULL,
    storage_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch >= 0),
    first_sequence INTEGER NOT NULL CHECK (first_sequence >= 0),
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= first_sequence),
    PRIMARY KEY (segment_id, storage_id, epoch),
    FOREIGN KEY (segment_id) REFERENCES journal_segments(segment_id) ON DELETE CASCADE
);
CREATE INDEX journal_segment_coverage_lookup
    ON journal_segment_coverage(storage_id, epoch, last_sequence);
CREATE TABLE journal_bases (
    base_id TEXT PRIMARY KEY,
    storage_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch >= 0),
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    object_key TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    committed_at INTEGER NOT NULL
);
CREATE INDEX journal_bases_storage_sequence
    ON journal_bases(storage_id, sequence);
CREATE TABLE journal_manifest_shards (
    shard_id TEXT PRIMARY KEY,
    shard_seq INTEGER NOT NULL UNIQUE CHECK (shard_seq >= 0),
    object_key TEXT NOT NULL UNIQUE,
    digest TEXT NOT NULL,
    encoded_bytes INTEGER NOT NULL CHECK (encoded_bytes >= 0),
    committed_at INTEGER NOT NULL
);
CREATE TABLE link_keyring (
    key_id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('primary','decrypt','retired')),
    created_at INTEGER NOT NULL,
    retired_at INTEGER
);
CREATE TABLE link_key_rotations (
    id TEXT PRIMARY KEY,
    from_key_id TEXT NOT NULL,
    to_key_id TEXT NOT NULL,
    cursor_slug TEXT,
    cursor_role TEXT,
    status TEXT NOT NULL CHECK (status IN ('prepared','running','committed','aborted')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX one_running_link_rotation ON link_key_rotations(status)
    WHERE status IN ('prepared','running');
CREATE TABLE object_accounting (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    object_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    version TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(storage_id, object_key)
);
CREATE TABLE object_reservations (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    object_key TEXT NOT NULL,
    old_bytes INTEGER NOT NULL CHECK (old_bytes >= 0),
    new_bytes INTEGER NOT NULL CHECK (new_bytes >= 0),
    created_at INTEGER NOT NULL,
    PRIMARY KEY(storage_id, operation_id, object_key)
);
CREATE INDEX object_reservations_age ON object_reservations(created_at, storage_id, operation_id);
CREATE INDEX documents_active_owner_updated
    ON documents(owner_id, updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL;
CREATE INDEX documents_active_owner_key_updated
    ON documents(owner_key, updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL AND owner_id IS NULL;
CREATE INDEX documents_active_example_updated
    ON documents(updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL AND example = 1;
CREATE INDEX documents_active_updated
    ON documents(updated_at DESC, slug DESC)
    WHERE status = 'active' AND pending_publication IS NULL;
CREATE INDEX grants_account_slug
    ON grants(account_id, slug);
CREATE INDEX guests_account_slug
    ON guests(account_id, slug, link_hash);
CREATE INDEX links_slug_hash_until
    ON links(slug, hash, until);
CREATE TABLE deletion_discovery (
    slug TEXT NOT NULL REFERENCES documents(slug) ON DELETE CASCADE,
    prefix TEXT NOT NULL,
    cursor TEXT,
    done INTEGER NOT NULL DEFAULT 0 CHECK (done IN (0,1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (slug, prefix)
);
CREATE INDEX deletion_discovery_pending ON deletion_discovery(slug, done, prefix);
CREATE INDEX journal_retirements_storage
    ON journal_retirements(storage_id, delete_after, object_key);
CREATE TABLE upload_buckets (
    owner_kind TEXT NOT NULL,
    owner_value TEXT NOT NULL,
    bucket INTEGER NOT NULL CHECK (bucket >= 0),
    uploads INTEGER NOT NULL CHECK (uploads >= 0),
    PRIMARY KEY (owner_kind, owner_value, bucket)
) WITHOUT ROWID;
CREATE INDEX upload_buckets_recent ON upload_buckets(bucket, owner_kind, owner_value);
CREATE TABLE journal_readers (
    reader_id TEXT PRIMARY KEY,
    object_key TEXT NOT NULL,
    opened_at INTEGER NOT NULL CHECK (opened_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= opened_at),
    heartbeat_at INTEGER NOT NULL CHECK (heartbeat_at >= opened_at)
);
CREATE INDEX journal_readers_object_expiry
    ON journal_readers(object_key, expires_at, reader_id);
CREATE INDEX checkpoints_by_account
    ON checkpoints (by_account, slug, sha)
    WHERE by_account IS NOT NULL;
CREATE TABLE quarto_selections (
    storage_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    render_id TEXT NOT NULL,
    source_revision TEXT NOT NULL,
    object_key TEXT NOT NULL,
    object_version TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (storage_id, document_id, context_id),
    FOREIGN KEY (storage_id) REFERENCES documents(storage_id) ON DELETE CASCADE
);
CREATE INDEX quarto_selections_by_storage
    ON quarto_selections (storage_id, document_id, context_id);
CREATE TABLE quarto_selection_epochs (
    storage_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (storage_id, document_id, context_id),
    FOREIGN KEY (storage_id) REFERENCES documents(storage_id) ON DELETE CASCADE
);
CREATE TABLE document_results_metadata (
    slug TEXT NOT NULL PRIMARY KEY REFERENCES documents(slug) ON DELETE CASCADE,
    execution_engine TEXT NOT NULL CHECK (execution_engine IN ('none', 'quarto')),
    draft_format TEXT NOT NULL CHECK (draft_format IN ('markdown', 'html', 'typst', 'latex', 'other'))
);
CREATE INDEX document_results_metadata_engine
    ON document_results_metadata(execution_engine, draft_format);
CREATE TRIGGER document_results_metadata_after_insert
AFTER INSERT ON documents
BEGIN
    INSERT INTO document_results_metadata (slug, execution_engine, draft_format)
    VALUES (
        NEW.slug,
        CASE WHEN NEW.source_format = 'quarto' THEN 'quarto' ELSE 'none' END,
        CASE NEW.source_format
            WHEN 'quarto' THEN 'markdown'
            WHEN 'markdown' THEN 'markdown'
            WHEN '' THEN 'html'
            WHEN 'html' THEN 'html'
            WHEN 'typst' THEN 'typst'
            WHEN 'latex' THEN 'latex'
            ELSE 'other'
        END
    );
END;
CREATE TRIGGER document_results_metadata_after_source_format_update
AFTER UPDATE OF source_format ON documents
BEGIN
    UPDATE document_results_metadata
    SET execution_engine = CASE WHEN NEW.source_format = 'quarto' THEN 'quarto' ELSE 'none' END,
        draft_format = CASE NEW.source_format
            WHEN 'quarto' THEN 'markdown'
            WHEN 'markdown' THEN 'markdown'
            WHEN '' THEN 'html'
            WHEN 'html' THEN 'html'
            WHEN 'typst' THEN 'typst'
            WHEN 'latex' THEN 'latex'
            ELSE 'other'
        END
    WHERE slug = NEW.slug;
END;
CREATE TABLE IF NOT EXISTS "account_examples" (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 4),
    slug TEXT NOT NULL UNIQUE,
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    PRIMARY KEY (account_id, position)
) WITHOUT ROWID;
CREATE TABLE quarto_selection_history (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    document_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    render_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    PRIMARY KEY (storage_id, document_id, context_id, render_id)
) WITHOUT ROWID;
CREATE TABLE agent_objects (
    slug TEXT NOT NULL REFERENCES documents(slug) ON DELETE CASCADE,
    actor TEXT NOT NULL,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY(slug, actor, id, kind)
);
CREATE INDEX agent_objects_expiry ON agent_objects(expires_at);
CREATE INDEX agent_receipts_expiry ON catalog_operations(created_at)
    WHERE kind IN ('agent_apply','agent_annotations','agent_cancel') AND status <> 'prepared';
CREATE TABLE agent_cancellations (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    target_request_id TEXT NOT NULL,
    cancel_request_id TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    status TEXT NOT NULL,
    result TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(storage_id, cancel_request_id),
    UNIQUE(storage_id, cancel_request_id)
);
CREATE INDEX agent_cancellations_expiry ON agent_cancellations(created_at);
CREATE INDEX agent_cancellations_target ON agent_cancellations(storage_id, target_request_id);
CREATE TABLE agent_execution_leases (
    slug TEXT NOT NULL REFERENCES documents(slug) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL,
    execution_epoch TEXT NOT NULL,
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    PRIMARY KEY (slug, conversation_id)
);
CREATE UNIQUE INDEX agent_execution_leases_epoch
    ON agent_execution_leases(slug, conversation_id, execution_epoch);
CREATE TABLE account_quota_preferences (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    payload TEXT NOT NULL CHECK (length(CAST(payload AS BLOB)) <= 65536),
    policy_generation TEXT NOT NULL,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
) WITHOUT ROWID;
CREATE TABLE source_history_encodings (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    file_digest TEXT NOT NULL,
    recipe_key TEXT NOT NULL,
    recipe_digest TEXT NOT NULL,
    codec INTEGER NOT NULL CHECK (codec > 0),
    uncompressed_bytes INTEGER NOT NULL CHECK (uncompressed_bytes >= 0),
    recipe_bytes INTEGER NOT NULL CHECK (recipe_bytes >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (storage_id, file_digest)
) WITHOUT ROWID;
CREATE TABLE source_history_objects (
    storage_id TEXT NOT NULL,
    file_digest TEXT NOT NULL,
    object_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    PRIMARY KEY (storage_id, file_digest, object_key),
    FOREIGN KEY (storage_id, file_digest)
        REFERENCES source_history_encodings(storage_id, file_digest)
        ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX source_history_objects_by_object
    ON source_history_objects(storage_id, object_key);
CREATE TABLE source_history_checkpoint_files (
    storage_id TEXT NOT NULL,
    checkpoint_sha TEXT NOT NULL,
    file_digest TEXT NOT NULL,
    PRIMARY KEY (storage_id, checkpoint_sha, file_digest),
    FOREIGN KEY (storage_id, file_digest)
        REFERENCES source_history_encodings(storage_id, file_digest)
        ON DELETE RESTRICT
) WITHOUT ROWID;
CREATE INDEX source_history_checkpoint_files_by_digest
    ON source_history_checkpoint_files(storage_id, file_digest);
CREATE TRIGGER source_history_document_removed
BEFORE DELETE ON documents
BEGIN
    DELETE FROM source_history_checkpoint_files WHERE storage_id=OLD.storage_id;
END;
CREATE TABLE source_history_write_leases (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    object_key TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= created_at),
    PRIMARY KEY (storage_id, operation_id, object_key)
) WITHOUT ROWID;
CREATE INDEX source_history_write_leases_expiry
    ON source_history_write_leases(expires_at, storage_id);
CREATE INDEX source_history_write_leases_by_object
    ON source_history_write_leases(storage_id, object_key);
CREATE TRIGGER source_history_checkpoint_removed
AFTER DELETE ON checkpoints
BEGIN
    DELETE FROM source_history_checkpoint_files
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND checkpoint_sha=OLD.sha;
    INSERT INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after)
    SELECT OLD.slug,o.object_key,o.bytes,unixepoch(),unixepoch()
      FROM source_history_objects o
      JOIN source_history_encodings e
        ON e.storage_id=o.storage_id AND e.file_digest=o.file_digest
     WHERE o.storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       -- An encoded chunk/recipe can be shared by more than one file digest.
       -- It is reclaimable only when no retained checkpoint names *any*
       -- encoding that owns this physical key.
       AND NOT EXISTS (SELECT 1
                      FROM source_history_objects o2
                      JOIN source_history_checkpoint_files r
                        ON r.storage_id=o2.storage_id
                       AND r.file_digest=o2.file_digest
                      WHERE o2.storage_id=o.storage_id
                        AND o2.object_key=o.object_key)
       AND NOT EXISTS (SELECT 1 FROM source_history_write_leases l
                      WHERE l.storage_id=o.storage_id AND l.object_key=o.object_key)
    ON CONFLICT(slug,object_key) DO UPDATE SET
      bytes=excluded.bytes,
      delete_after=MIN(pending_deletes.delete_after,excluded.delete_after);
    DELETE FROM source_history_encodings
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND NOT EXISTS (SELECT 1 FROM source_history_checkpoint_files r
                      WHERE r.storage_id=source_history_encodings.storage_id
                        AND r.file_digest=source_history_encodings.file_digest)
       AND NOT EXISTS (SELECT 1
                      FROM source_history_objects o
                      JOIN source_history_write_leases l
                        ON l.storage_id=o.storage_id
                       AND l.object_key=o.object_key
                      WHERE o.storage_id=source_history_encodings.storage_id
                        AND o.file_digest=source_history_encodings.file_digest);
END;
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
CREATE TABLE document_retention_policy (
    slug TEXT PRIMARY KEY REFERENCES documents(slug) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK (mode IN ('balanced', 'custom')),
    policy_version INTEGER NOT NULL DEFAULT 1 CHECK (policy_version >= 1),
    enrolled_at INTEGER NOT NULL CHECK (enrolled_at >= 0),
    last_scheduled_at INTEGER NOT NULL DEFAULT 0 CHECK (last_scheduled_at >= 0)
) WITHOUT ROWID;
CREATE INDEX renderings_publication_order
    ON renderings(slug, published_seq DESC, at DESC, tree_sha DESC);
CREATE TABLE checkpoint_asset_refs (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    checkpoint_sha TEXT NOT NULL,
    object_key TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes >= 0),
    PRIMARY KEY (storage_id, checkpoint_sha, object_key)
) WITHOUT ROWID;
CREATE INDEX checkpoint_asset_refs_by_object
    ON checkpoint_asset_refs(storage_id, object_key);
CREATE TABLE checkpoint_asset_sets (
    storage_id TEXT NOT NULL REFERENCES documents(storage_id) ON DELETE CASCADE,
    checkpoint_sha TEXT NOT NULL,
    asset_count INTEGER NOT NULL CHECK (asset_count >= 0),
    PRIMARY KEY (storage_id, checkpoint_sha)
) WITHOUT ROWID;
CREATE TRIGGER checkpoint_asset_removed
AFTER DELETE ON checkpoints
BEGIN
    -- Queueing is deliberately not physical deletion. The ledger stays
    -- charged until the deletion worker confirms physical reclamation.
    INSERT INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after)
    SELECT OLD.slug,r.object_key,r.bytes,unixepoch(),unixepoch()
      FROM checkpoint_asset_refs r
     WHERE r.storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND r.checkpoint_sha=OLD.sha
       AND NOT EXISTS (
           SELECT 1
             FROM checkpoint_asset_refs retained
             JOIN checkpoints c ON c.slug=OLD.slug
                              AND c.sha=retained.checkpoint_sha
            WHERE retained.storage_id=r.storage_id
              AND retained.object_key=r.object_key
       )
       AND NOT EXISTS (
           SELECT 1 FROM source_history_write_leases l
            WHERE l.storage_id=r.storage_id AND l.object_key=r.object_key
       )
       -- If the live journal is ahead of the remaining durable checkpoints,
       -- do not even create a pending row: the next publication must be able
       -- to lease this asset. The object ledger remains charged until a later
       -- live-root reconciliation can queue it safely.
       AND COALESCE((
             SELECT MAX(c.last_sequence)
               FROM journal_segment_coverage c
              WHERE c.storage_id=r.storage_id OR c.storage_id=''
       ),0) <= COALESCE((
             SELECT MAX(c.durable_seq)
               FROM checkpoints c
              WHERE c.slug=OLD.slug
       ),0)
       AND COALESCE((
             SELECT MAX(b.sequence)
               FROM journal_bases b
              WHERE b.storage_id=r.storage_id OR b.storage_id=''
       ),0) <= COALESCE((
             SELECT MAX(c.durable_seq)
               FROM checkpoints c
              WHERE c.slug=OLD.slug
       ),0)
    ON CONFLICT(slug,object_key) DO UPDATE SET
      bytes=excluded.bytes,
      delete_after=MIN(pending_deletes.delete_after,excluded.delete_after);
    DELETE FROM checkpoint_asset_refs
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND checkpoint_sha=OLD.sha;
    DELETE FROM checkpoint_asset_sets
     WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=OLD.slug)
       AND checkpoint_sha=OLD.sha;
END;
