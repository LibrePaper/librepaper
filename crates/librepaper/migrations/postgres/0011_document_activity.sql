-- When a document was worked on, kept past the compaction that deletes the
-- updates it was measured from, and anchored so a reader can go there.
--
-- `document_updates` is a queue: once a base holds those operations, the rows
-- are deleted (storage/postgres/collaboration.rs). Their `created_at` is the
-- only trustworthy clock the system has -- it is stamped server-side at
-- admission, unlike a Loro change timestamp, which is the client's and is
-- forced monotonic -- so it is aggregated into buckets here before the delete
-- rather than lost with them.
--
-- The anchor is a Loro frontier rather than an update sequence, because the
-- sequence means nothing once the rows behind it are gone: the base is the
-- whole operation history, and `LoroDoc::fork_at(frontier)` is what turns a
-- moment on this timeline into the document as it stood then. Recorded by the
-- room, which is the only place that holds the document.
ALTER TABLE document_updates ADD COLUMN frontier bytea NOT NULL DEFAULT ''::bytea;

-- One row per document, bucket and peer, not one per write: a million
-- keystrokes is a few thousand minutes. `peer` is in the key so per-author
-- activity needs no migration later; nothing records an author on an update
-- today, and those rows carry the empty string.
--
-- `state_bytes` is the largest encoded history seen in the bucket, not the
-- weight of the edits in it: a persisted row is the whole operation history
-- (`session::encode_state`), so its size measures the document, and the work
-- done in a bucket is the difference between consecutive buckets. `changes`
-- counts persistence cycles -- roughly one per couple of seconds in which
-- somebody was typing -- and is the honest count of "was this minute worked
-- in".
CREATE TABLE document_activity (
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    bucket timestamptz NOT NULL,
    peer text NOT NULL,
    changes integer NOT NULL CHECK (changes >= 0),
    state_bytes bigint NOT NULL CHECK (state_bytes >= 0),
    frontier bytea NOT NULL,
    PRIMARY KEY (document_id, bucket, peer)
);
