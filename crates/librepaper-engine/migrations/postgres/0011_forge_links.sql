-- Forge links. One row per project connected to a git forge. The row keeps
-- what the browser needs to plan a pull or push: the repository and branch,
-- the commit and per-file map it last synced, and the export branch it
-- staged, if any. Every write carries the revision the browser read, so two
-- tabs cannot record baselines out of order.
--
-- The link goes with the document. A purge removes the document and with it
-- the link; a trash keeps both, like marks and tags.
CREATE TABLE forge_links (
    document_id uuid PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    provider text NOT NULL CHECK (provider IN ('github')),
    repository_id bigint NOT NULL,
    repository text NOT NULL,
    branch text NOT NULL,
    folder text NOT NULL DEFAULT '',
    connected_by uuid REFERENCES accounts(id) ON DELETE SET NULL,
    synced_commit text,
    synced_map jsonb NOT NULL DEFAULT '{}'::jsonb,
    export jsonb,
    revision bigint NOT NULL DEFAULT 1 CHECK (revision >= 1),
    last_pull_at timestamptz,
    last_push_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
