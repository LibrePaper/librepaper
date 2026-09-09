ALTER TABLE links ADD COLUMN key_id TEXT NOT NULL DEFAULT 'legacy';

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
