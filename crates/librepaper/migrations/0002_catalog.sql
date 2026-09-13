-- Proposed catalog v2 schema. This is a specification artifact, not an
-- application migration. See sql-schema-v2.md for the transaction contracts.
-- Fresh databases only. Execute with foreign_keys enabled, in a transaction.
-- All times are Unix milliseconds. No application-defined SQL functions.
-- DDL only: the initializer inserts deployment-specific server_state and sets
-- PRAGMA user_version=2 before committing the SAME transaction. Loading this
-- file alone deliberately leaves user_version unchanged.

CREATE TABLE accounts (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 128),
    kind TEXT NOT NULL CHECK (kind IN ('registered', 'anonymous', 'system')),
    provider TEXT,
    provider_subject TEXT,
    handle TEXT NOT NULL,
    display_name TEXT NOT NULL,
    email TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'erasing', 'blocked')),
    session_generation TEXT NOT NULL,
    plan TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    last_seen_at INTEGER NOT NULL CHECK (last_seen_at >= 0),
    last_active_at INTEGER CHECK (last_active_at >= 0),
    preferences_revision INTEGER NOT NULL DEFAULT 0 CHECK (preferences_revision >= 0),
    preferences_json TEXT NOT NULL DEFAULT '{"version":1}'
        CHECK (json_valid(preferences_json) AND length(CAST(preferences_json AS BLOB)) <= 65536),
    bookmarks_json TEXT NOT NULL DEFAULT '{"version":1,"items":[]}'
        CHECK (json_valid(bookmarks_json) AND length(CAST(bookmarks_json AS BLOB)) <= 262144),
    onboarding_json TEXT NOT NULL DEFAULT '{"version":1,"items":[]}'
        CHECK (json_valid(onboarding_json) AND length(CAST(onboarding_json AS BLOB)) <= 16384),
    stored_bytes INTEGER NOT NULL DEFAULT 0 CHECK (stored_bytes >= 0),
    reserved_bytes INTEGER NOT NULL DEFAULT 0 CHECK (reserved_bytes >= 0),
    document_count INTEGER NOT NULL DEFAULT 0 CHECK (document_count >= 0),
    CHECK ((kind = 'registered' AND provider IS NOT NULL AND provider_subject IS NOT NULL)
        OR (kind <> 'registered' AND provider IS NULL AND provider_subject IS NULL))
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX accounts_identity ON accounts(provider, provider_subject)
    WHERE kind = 'registered';
CREATE INDEX accounts_erasing ON accounts(id) WHERE status = 'erasing';

CREATE TABLE documents (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 128),
    slug TEXT NOT NULL UNIQUE CHECK (length(slug) BETWEEN 1 AND 256),
    owner_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE RESTRICT,
    ownership_mode TEXT NOT NULL CHECK (ownership_mode IN ('owned', 'open', 'example')),
    title TEXT NOT NULL CHECK (length(CAST(title AS BLOB)) <= 4096),
    title_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('creating', 'active', 'deleting')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    published_at INTEGER CHECK (published_at >= 0),
    source_format TEXT NOT NULL CHECK (source_format IN ('markdown','html','typst','latex','quarto')),
    main_path TEXT NOT NULL,
    settings_revision INTEGER NOT NULL DEFAULT 0 CHECK (settings_revision >= 0),
    settings_json TEXT NOT NULL DEFAULT '{"version":1}'
        CHECK (json_valid(settings_json) AND length(CAST(settings_json AS BLOB)) <= 65536),
    retention_mode TEXT NOT NULL DEFAULT 'balanced' CHECK (retention_mode IN ('balanced','manual')),
    retention_revision INTEGER NOT NULL DEFAULT 0 CHECK (retention_revision >= 0),
    retention_json TEXT NOT NULL DEFAULT '{"version":1}'
        CHECK (json_valid(retention_json) AND length(CAST(retention_json AS BLOB)) <= 16384),
    last_checkpoint_at INTEGER NOT NULL DEFAULT 0 CHECK (last_checkpoint_at >= 0),
    retention_due_at INTEGER NOT NULL DEFAULT 0 CHECK (retention_due_at >= 0),
    next_annotation_seq INTEGER NOT NULL DEFAULT 1 CHECK (next_annotation_seq >= 1),
    next_checkpoint_seq INTEGER NOT NULL DEFAULT 1 CHECK (next_checkpoint_seq >= 1),
    source_generation INTEGER NOT NULL DEFAULT 0 CHECK (source_generation >= 0),
    journal_epoch INTEGER NOT NULL DEFAULT 0 CHECK (journal_epoch >= 0),
    journal_sequence INTEGER NOT NULL DEFAULT 0 CHECK (journal_sequence >= 0),
    journal_base_sequence INTEGER NOT NULL DEFAULT 0
        CHECK (journal_base_sequence >= 0 AND journal_base_sequence <= journal_sequence),
    journal_base_object_id TEXT,
    current_checkpoint_id TEXT,
    publication_id TEXT,
    publication_object_id TEXT,
    stored_bytes INTEGER NOT NULL DEFAULT 0 CHECK (stored_bytes >= 0),
    reserved_bytes INTEGER NOT NULL DEFAULT 0 CHECK (reserved_bytes >= 0),
    agent_payload_bytes INTEGER NOT NULL DEFAULT 0 CHECK (agent_payload_bytes BETWEEN 0 AND 33554432),
    agent_payload_count INTEGER NOT NULL DEFAULT 0 CHECK (agent_payload_count BETWEEN 0 AND 512),
    checkpoint_ref_count INTEGER NOT NULL DEFAULT 0 CHECK (checkpoint_ref_count BETWEEN 0 AND 1048576),
    CHECK ((publication_id IS NULL AND publication_object_id IS NULL AND published_at IS NULL)
        OR (publication_id IS NOT NULL AND publication_object_id IS NOT NULL AND published_at IS NOT NULL)),
    CHECK (journal_base_object_id IS NOT NULL OR journal_base_sequence = 0),
    FOREIGN KEY (id, current_checkpoint_id) REFERENCES checkpoints(document_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (id, journal_base_object_id) REFERENCES objects(document_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (id, publication_object_id) REFERENCES objects(document_id, id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX documents_title ON documents(owner_id, title_key) WHERE status <> 'deleting';
CREATE INDEX documents_owner_list ON documents(owner_id, status, updated_at DESC, id DESC);
CREATE INDEX documents_examples ON documents(updated_at DESC, id DESC)
    WHERE status = 'active' AND ownership_mode = 'example';
CREATE INDEX documents_checkpoint_due ON documents(last_checkpoint_at, id) WHERE status = 'active';
CREATE INDEX documents_retention_due ON documents(retention_due_at, id) WHERE status = 'active';
CREATE INDEX documents_deleting ON documents(id) WHERE status = 'deleting';

CREATE TABLE grants (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('reader','commenter','editor')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (document_id, account_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX grants_account ON grants(account_id, document_id);

CREATE TABLE links (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('reader','commenter','editor')),
    token_hash TEXT NOT NULL UNIQUE CHECK (length(token_hash) = 64),
    sealed_token BLOB NOT NULL,
    sealing_key_id TEXT NOT NULL,
    credential_generation INTEGER NOT NULL DEFAULT 1 CHECK (credential_generation >= 1),
    label TEXT NOT NULL,
    budget INTEGER CHECK (budget >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER CHECK (expires_at >= created_at),
    PRIMARY KEY (document_id, id),
    UNIQUE (document_id, role)
) STRICT, WITHOUT ROWID;

CREATE TABLE annotations (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    seq INTEGER NOT NULL CHECK (seq >= 1),
    kind TEXT NOT NULL CHECK (kind IN ('comment','highlight','suggestion')),
    body TEXT NOT NULL CHECK (length(CAST(body AS BLOB)) <= 65536),
    author_account_id TEXT REFERENCES accounts(id) ON DELETE RESTRICT,
    author_key TEXT NOT NULL,
    author_label TEXT NOT NULL,
    via TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    publication_id TEXT,
    source_revision TEXT,
    selector_json TEXT NOT NULL CHECK (json_valid(selector_json)
        AND length(CAST(selector_json AS BLOB)) <= 65536),
    context_json TEXT NOT NULL DEFAULT '{"version":1}' CHECK (json_valid(context_json)
        AND length(CAST(context_json AS BLOB)) <= 16384),
    protected_checkpoint_id TEXT,
    proposed_text TEXT CHECK (length(CAST(proposed_text AS BLOB)) <= 1048576),
    suggestion_state TEXT CHECK (suggestion_state IN ('proposed','accepted','rejected')),
    acceptance_operation_id TEXT,
    resolution_revision TEXT,
    resolved_at INTEGER CHECK (resolved_at >= created_at),
    PRIMARY KEY (document_id, id),
    UNIQUE (document_id, seq),
    CHECK ((kind = 'suggestion' AND proposed_text IS NOT NULL AND suggestion_state IS NOT NULL)
        OR (kind <> 'suggestion' AND proposed_text IS NULL AND suggestion_state IS NULL)),
    CHECK (suggestion_state IS NOT 'accepted'
        OR (acceptance_operation_id IS NOT NULL AND resolution_revision IS NOT NULL AND resolved_at IS NOT NULL)),
    FOREIGN KEY (document_id, protected_checkpoint_id)
        REFERENCES checkpoints(document_id, id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX annotations_author ON annotations(author_account_id, document_id, id)
    WHERE author_account_id IS NOT NULL;
CREATE INDEX annotations_protection ON annotations(document_id, protected_checkpoint_id)
    WHERE protected_checkpoint_id IS NOT NULL;

CREATE TABLE replies (
    document_id TEXT NOT NULL,
    annotation_id TEXT NOT NULL,
    id TEXT NOT NULL,
    body TEXT NOT NULL CHECK (length(CAST(body AS BLOB)) <= 65536),
    author_account_id TEXT REFERENCES accounts(id) ON DELETE RESTRICT,
    author_key TEXT NOT NULL,
    author_label TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    PRIMARY KEY (document_id, annotation_id, id),
    FOREIGN KEY (document_id, annotation_id) REFERENCES annotations(document_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
CREATE INDEX replies_thread ON replies(document_id, annotation_id, created_at, id);
CREATE INDEX replies_author ON replies(author_account_id, document_id, annotation_id, id)
    WHERE author_account_id IS NOT NULL;

CREATE TABLE checkpoints (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE RESTRICT,
    id TEXT NOT NULL,
    seq INTEGER NOT NULL CHECK (seq >= 1),
    tree_object_id TEXT NOT NULL,
    tree_digest TEXT NOT NULL CHECK (length(tree_digest) = 64),
    parent_id TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    author_account_id TEXT REFERENCES accounts(id) ON DELETE RESTRICT,
    author_label TEXT NOT NULL,
    reason TEXT NOT NULL,
    source_format TEXT NOT NULL CHECK (source_format IN ('markdown','html','typst','latex','quarto')),
    logical_bytes INTEGER NOT NULL CHECK (logical_bytes >= 0),
    label TEXT,
    journal_epoch INTEGER NOT NULL CHECK (journal_epoch >= 0),
    journal_sequence INTEGER NOT NULL CHECK (journal_sequence >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{"version":1}' CHECK (json_valid(metadata_json)
        AND length(CAST(metadata_json AS BLOB)) <= 65536),
    eligible_after INTEGER CHECK (eligible_after >= 0),
    PRIMARY KEY (document_id, id),
    UNIQUE (document_id, seq),
    FOREIGN KEY (document_id, tree_object_id) REFERENCES objects(document_id, id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX checkpoints_author ON checkpoints(author_account_id, document_id, id)
    WHERE author_account_id IS NOT NULL;
CREATE INDEX checkpoints_expiry ON checkpoints(document_id, eligible_after, seq)
    WHERE eligible_after IS NOT NULL;
CREATE INDEX checkpoints_tree ON checkpoints(document_id, tree_object_id);

CREATE TABLE objects (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE RESTRICT,
    id TEXT NOT NULL,
    storage_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('source_chunk','source_recipe','source_tree','asset',
        'publication_manifest','publication_html','publication_asset','journal_segment','journal_base','agent_payload')),
    state TEXT NOT NULL CHECK (state IN ('allocated','available','deleting')),
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    logical_digest TEXT CHECK (length(logical_digest) = 64),
    encoding_version INTEGER NOT NULL DEFAULT 1 CHECK (encoding_version >= 1),
    byte_length INTEGER CHECK (byte_length >= 0),
    reserved_bytes INTEGER NOT NULL CHECK (reserved_bytes >= 0),
    allocation_operation_id TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    live_root INTEGER NOT NULL DEFAULT 0 CHECK (live_root IN (0,1)),
    publication_root INTEGER NOT NULL DEFAULT 0 CHECK (publication_root IN (0,1)),
    gc_after INTEGER CHECK (gc_after >= 0),
    retry_at INTEGER CHECK (retry_at >= 0),
    journal_epoch INTEGER,
    first_sequence INTEGER,
    last_sequence INTEGER,
    PRIMARY KEY (document_id, id),
    CHECK ((state = 'allocated' AND byte_length IS NULL AND allocation_operation_id IS NOT NULL)
        OR (state = 'available' AND byte_length IS NOT NULL AND reserved_bytes = 0
            AND allocation_operation_id IS NULL)
        OR state = 'deleting'),
    -- Even an unknown-length zero-byte reservation needs an allocation owner.
    -- Known empty objects are valid: byte_length=0 is different from NULL.
    CHECK (byte_length IS NOT NULL OR allocation_operation_id IS NOT NULL),
    CHECK (byte_length IS NULL OR (reserved_bytes = 0 AND allocation_operation_id IS NULL)),
    CHECK (state = 'available' OR (live_root = 0 AND publication_root = 0)),
    CHECK (state <> 'deleting' OR retry_at IS NOT NULL),
    CHECK ((kind IN ('journal_segment','journal_base') AND journal_epoch IS NOT NULL
            AND first_sequence IS NOT NULL AND last_sequence IS NOT NULL
            AND journal_epoch >= 0 AND first_sequence >= 0 AND last_sequence >= first_sequence)
        OR (kind NOT IN ('journal_segment','journal_base') AND journal_epoch IS NULL
            AND first_sequence IS NULL AND last_sequence IS NULL)),
    FOREIGN KEY (document_id, allocation_operation_id) REFERENCES operations(document_id, id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX objects_reuse ON objects(document_id, kind, digest, encoding_version)
    WHERE state = 'available';
CREATE INDEX objects_recipe_reuse ON objects(document_id, logical_digest, encoding_version)
    WHERE state = 'available' AND kind = 'source_recipe';
CREATE INDEX objects_gc ON objects(gc_after, document_id, id)
    WHERE state = 'available' AND live_root = 0 AND publication_root = 0;
CREATE INDEX objects_delete ON objects(retry_at, document_id, id) WHERE state = 'deleting';
CREATE INDEX objects_journal ON objects(document_id, journal_epoch, first_sequence, id)
    WHERE kind IN ('journal_segment','journal_base') AND state = 'available';
CREATE INDEX objects_allocation ON objects(allocation_operation_id, document_id, id)
    WHERE allocation_operation_id IS NOT NULL;

CREATE TABLE checkpoint_objects (
    document_id TEXT NOT NULL,
    checkpoint_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    PRIMARY KEY (document_id, checkpoint_id, object_id),
    FOREIGN KEY (document_id, checkpoint_id) REFERENCES checkpoints(document_id, id) ON DELETE CASCADE,
    FOREIGN KEY (document_id, object_id) REFERENCES objects(document_id, id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX checkpoint_objects_object ON checkpoint_objects(document_id, object_id, checkpoint_id);

CREATE TABLE object_leases (
    document_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    holder_id TEXT NOT NULL,
    purpose TEXT NOT NULL CHECK (purpose IN ('read','write','stage')),
    operation_id TEXT,
    writer_generation TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at >= created_at),
    PRIMARY KEY (document_id, object_id, holder_id),
    CHECK (purpose = 'read' OR operation_id IS NOT NULL),
    FOREIGN KEY (document_id, object_id) REFERENCES objects(document_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (document_id, operation_id) REFERENCES operations(document_id, id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX object_leases_expiry ON object_leases(expires_at, document_id, object_id, holder_id);
CREATE INDEX object_leases_operation ON object_leases(operation_id, document_id, object_id)
    WHERE operation_id IS NOT NULL;

CREATE TABLE operations (
    id TEXT NOT NULL PRIMARY KEY,
    document_id TEXT REFERENCES documents(id) ON DELETE RESTRICT,
    account_id TEXT REFERENCES accounts(id) ON DELETE RESTRICT,
    actor_key TEXT NOT NULL,
    request_key TEXT NOT NULL CHECK (length(CAST(request_key AS BLOB)) BETWEEN 1 AND 128),
    kind TEXT NOT NULL CHECK (kind IN ('source_publish','display_publish','checkpoint','checkpoint_delete',
        'journal_append','journal_compact','agent_apply','agent_annotations','agent_cancel',
        'agent_execution','agent_stage','erase_account','erase_document','rotate_links','backup')),
    request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
    state TEXT NOT NULL CHECK (state IN ('prepared','committed','aborted')),
    writer_generation TEXT NOT NULL,
    expected_document_generation INTEGER CHECK (expected_document_generation >= 0),
    target_operation_id TEXT,
    target_request_key TEXT,
    conversation_id TEXT,
    execution_epoch TEXT,
    plan_json TEXT NOT NULL DEFAULT '{"version":1}' CHECK (json_valid(plan_json)
        AND length(CAST(plan_json AS BLOB)) <= 65536),
    result_json TEXT CHECK (json_valid(result_json) AND length(CAST(result_json AS BLOB)) <= 65536),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    work_expires_at INTEGER CHECK (work_expires_at >= created_at),
    receipt_expires_at INTEGER CHECK (receipt_expires_at >= created_at),
    completed_at INTEGER CHECK (completed_at >= created_at),
    CHECK ((kind = 'erase_account' AND account_id IS NOT NULL AND document_id IS NULL)
        OR (kind IN ('rotate_links','backup') AND document_id IS NULL AND account_id IS NULL)
        OR (kind IN ('source_publish','display_publish','checkpoint','checkpoint_delete',
            'journal_append','journal_compact','agent_apply','agent_annotations','agent_cancel',
            'agent_execution','agent_stage','erase_document')
            AND document_id IS NOT NULL AND account_id IS NULL)),
    CHECK ((state = 'prepared' AND completed_at IS NULL)
        OR (state <> 'prepared' AND completed_at IS NOT NULL AND result_json IS NOT NULL)),
    CHECK (state = 'prepared' OR receipt_expires_at IS NOT NULL),
    CHECK (completed_at IS NULL OR receipt_expires_at >= completed_at),
    CHECK (state <> 'prepared' OR work_expires_at IS NOT NULL
        OR kind IN ('erase_account','erase_document','rotate_links','backup','journal_compact')),
    CHECK (kind <> 'agent_execution' OR (document_id IS NOT NULL AND conversation_id IS NOT NULL
        AND execution_epoch IS NOT NULL AND work_expires_at IS NOT NULL)),
    CHECK (kind <> 'agent_cancel' OR target_request_key IS NOT NULL),
    UNIQUE (document_id, id)
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX operations_request_document ON operations(document_id, actor_key, request_key)
    WHERE document_id IS NOT NULL;
CREATE UNIQUE INDEX operations_request_account ON operations(account_id, actor_key, request_key)
    WHERE account_id IS NOT NULL;
CREATE UNIQUE INDEX operations_request_server ON operations(actor_key, request_key)
    WHERE document_id IS NULL AND account_id IS NULL;
CREATE INDEX operations_document ON operations(document_id, state, id) WHERE document_id IS NOT NULL;
CREATE INDEX operations_account ON operations(account_id, state, id) WHERE account_id IS NOT NULL;
CREATE INDEX operations_recovery ON operations(kind, updated_at, id) WHERE state = 'prepared';
CREATE INDEX operations_expiry ON operations(receipt_expires_at, id) WHERE state <> 'prepared';
CREATE INDEX operations_work_expiry ON operations(work_expires_at, id) WHERE state = 'prepared';
CREATE INDEX operations_actor ON operations(actor_key, id);
CREATE INDEX operations_cancel_target ON operations(document_id, actor_key, target_request_key)
    WHERE kind = 'agent_cancel';
CREATE UNIQUE INDEX operations_execution ON operations(document_id, conversation_id)
    WHERE kind = 'agent_execution' AND state = 'prepared';
CREATE UNIQUE INDEX operations_epoch ON operations(document_id, execution_epoch)
    WHERE kind = 'agent_execution';
CREATE UNIQUE INDEX operations_server_work ON operations(kind)
    WHERE document_id IS NULL AND account_id IS NULL AND state = 'prepared';
CREATE UNIQUE INDEX operations_document_writer ON operations(document_id)
    WHERE state = 'prepared' AND kind IN ('source_publish','checkpoint','journal_append','journal_compact','agent_apply');
CREATE UNIQUE INDEX operations_erase_account ON operations(account_id)
    WHERE kind = 'erase_account' AND state = 'prepared';
CREATE UNIQUE INDEX operations_erase_document ON operations(document_id)
    WHERE kind = 'erase_document' AND state = 'prepared';

CREATE TABLE server_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    deployment_id TEXT NOT NULL,
    writer_generation TEXT NOT NULL,
    catalog_revision INTEGER NOT NULL DEFAULT 0 CHECK (catalog_revision >= 0),
    stored_bytes INTEGER NOT NULL DEFAULT 0 CHECK (stored_bytes >= 0),
    reserved_bytes INTEGER NOT NULL DEFAULT 0 CHECK (reserved_bytes >= 0),
    document_count INTEGER NOT NULL DEFAULT 0 CHECK (document_count >= 0),
    agent_payload_bytes INTEGER NOT NULL DEFAULT 0 CHECK (agent_payload_bytes BETWEEN 0 AND 134217728),
    agent_payload_count INTEGER NOT NULL DEFAULT 0 CHECK (agent_payload_count BETWEEN 0 AND 16384),
    checkpoint_ref_count INTEGER NOT NULL DEFAULT 0 CHECK (checkpoint_ref_count BETWEEN 0 AND 8388608),
    active_link_key_id TEXT NOT NULL,
    keyring_json TEXT NOT NULL CHECK (json_valid(keyring_json)
        AND length(CAST(keyring_json AS BLOB)) <= 16384),
    cost_json TEXT NOT NULL CHECK (json_valid(cost_json)
        AND length(CAST(cost_json AS BLOB)) <= 262144),
    maintenance_json TEXT NOT NULL DEFAULT '{"version":1}' CHECK (json_valid(maintenance_json)
        AND length(CAST(maintenance_json AS BLOB)) <= 16384),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;
