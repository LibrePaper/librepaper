-- A custom template is hidden from ordinary listings as soon as its marker
-- is written. The operation row makes it visible on the template shelf only
-- after the copied source and its first durable label commit together.
CREATE TABLE document_template_operations (
    owner_id uuid NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    source_document_id uuid NOT NULL,
    title text NOT NULL,
    target_document_id uuid NOT NULL UNIQUE REFERENCES documents(id) ON DELETE CASCADE,
    completed_at timestamptz,
    source_sequence bigint CHECK (source_sequence IS NULL OR source_sequence >= 0),
    tree_digest bytea CHECK (tree_digest IS NULL OR octet_length(tree_digest) = 32),
    PRIMARY KEY (owner_id, request_id),
    CHECK ((completed_at IS NULL) = (source_sequence IS NULL)),
    CHECK ((completed_at IS NULL) = (tree_digest IS NULL))
);
