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
    until TEXT NOT NULL,
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
    changed TEXT,
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
    resolved_in TEXT NOT NULL,
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

CREATE TABLE conversations (
    slug TEXT NOT NULL REFERENCES documents (slug) ON DELETE CASCADE,
    id TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (slug, id)
) WITHOUT ROWID;
CREATE INDEX conversations_expiry ON conversations (expires_at);

CREATE TABLE messages (
    slug TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    cursor INTEGER NOT NULL,
    id TEXT NOT NULL,
    role TEXT NOT NULL,
    text TEXT NOT NULL,
    context TEXT,
    PRIMARY KEY (slug, conversation_id, cursor),
    FOREIGN KEY (slug, conversation_id)
        REFERENCES conversations (slug, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX messages_request ON messages (slug, conversation_id, id);

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
    synctex_bytes INTEGER NOT NULL,
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

INSERT INTO totals (id, bytes, documents) VALUES (1, 0, 0);
