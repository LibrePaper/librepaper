-- Sync links. One row per project connected to a git remote. The row keeps
-- what every desktop needs to plan a pull or push: the remote's label, the
-- branch and folder, the commit last incorporated and the project paths that
-- were synced at it. The remote URL and the credentials stay on the desktop.
-- Every write carries the revision the caller read, so two tabs cannot record
-- a checkpoint out of order.
--
-- The link goes with the document. A purge removes the document and with it
-- the link; a trash keeps both, like marks and tags.
CREATE TABLE sync_links (
    document_id uuid PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    revision bigint NOT NULL DEFAULT 1 CHECK (revision >= 1),
    remote_kind text NOT NULL CHECK (remote_kind IN ('network', 'local')),
    remote_label text NOT NULL,
    connection_id uuid NOT NULL,
    branch text NOT NULL,
    folder text NOT NULL DEFAULT '',
    checkpoint text,
    synced_paths jsonb NOT NULL DEFAULT '[]'::jsonb,
    connected_by uuid REFERENCES accounts(id) ON DELETE SET NULL,
    last_pull_at timestamptz,
    last_pull_by uuid REFERENCES accounts(id) ON DELETE SET NULL,
    last_push_at timestamptz,
    last_push_by uuid REFERENCES accounts(id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
