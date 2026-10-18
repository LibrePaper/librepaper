-- Project chat history. Messages belong to the project and count toward the
-- owner's storage through documents.chat_bytes (body bytes plus a fixed row
-- overhead per message). Deleting the project deletes its chat.
CREATE TABLE chat_messages (
    id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    author_account_id uuid REFERENCES accounts(id) ON DELETE SET NULL,
    author_label text NOT NULL,
    body text NOT NULL,
    created_at timestamptz NOT NULL
);
CREATE INDEX chat_messages_document_id_id ON chat_messages(document_id, id);
ALTER TABLE documents ADD COLUMN chat_bytes bigint NOT NULL DEFAULT 0 CHECK (chat_bytes >= 0);
