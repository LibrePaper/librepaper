-- Charge writes only for the reads that exist.
--
-- Every index and every column is paid for on each insert and update, forever,
-- whether or not a query ever reads it. This migration removes the ones no
-- query in the tree reads, and adds the ones the hot paths were missing --
-- most importantly the referencing-side indexes without which deleting a row
-- scans whole tables.

-- 1. Title normalization nothing searches.
--
-- `title_key` was specified as the case-folded key "for search and sorting",
-- and the v2 schema made it the owner-local uniqueness key. v3 kept the column
-- and its index but dropped the uniqueness rule, and no title search was ever
-- built: the column is written on create and rename, projected by seven
-- SELECTs, and never appears in a WHERE, ORDER BY, or constraint. It is a pure
-- function of `title`, so a future search feature can add it back and backfill
-- it in one statement.
ALTER TABLE documents DROP COLUMN title_key;  -- drops documents_title_search

-- 2. The collaboration update log, which is the highest-write table here.
--
-- `document_updates_replay` was created with exactly the column list the
-- UNIQUE constraint already indexes, so every append maintained the same btree
-- twice. The surrogate `id` is equally unpaid-for: ordering, replay, and
-- compaction all address a row as (document_id, update_sequence), and nothing
-- outside the row struct ever read `id`. Making that pair the primary key
-- leaves one btree on a table that was carrying three.
--
-- The table holds only updates not yet folded into a base snapshot, so the
-- rebuild below is bounded by the compaction threshold rather than by history.
DROP INDEX document_updates_replay;
ALTER TABLE document_updates
    DROP CONSTRAINT document_updates_document_id_update_sequence_key,
    DROP CONSTRAINT document_updates_pkey,
    DROP COLUMN id,
    ADD PRIMARY KEY (document_id, update_sequence);

-- 3. Indexes whose every query is already served by another index.
--
-- `sequence` is unique within a document, so the trailing `id DESC` in the
-- version-history index resolves no tie the UNIQUE (document_id, sequence)
-- index cannot resolve by scanning backwards; the two history queries drop
-- that tiebreak from their ORDER BY in the same change. Assets are only ever
-- read by (document_id) and (document_id, digest) -- never in created_at order
-- -- which the UNIQUE (document_id, digest, byte_length) index covers, and
-- publications only by (document_id, request_key) and by id.
DROP INDEX document_versions_history;
DROP INDEX document_assets_document;
DROP INDEX publications_document;

-- 4. The global listing index, pointed at the documents that are listed.
--
-- `documents_open_updated` indexed active *open* documents in listing order,
-- but no query has ever filtered on ownership_mode='open' -- the one listing
-- predicate that survived is 'example', inside visible_documents. Meanwhile
-- list_documents and visible_documents page every active document by
-- (updated_at, id) with no index in that order at all, and so sorted the table
-- on each page. One partial index on the real predicate serves both.
DROP INDEX documents_open_updated;
CREATE INDEX documents_active_updated ON documents(updated_at DESC, id DESC)
    WHERE status = 'active';

-- Note: documents_owner_updated deliberately stays unpartitioned. Account
-- erasure counts an owner's documents without a status filter, so restricting
-- that index to active rows would cost the scan it saves.

-- 5. Referencing columns of ON DELETE SET NULL foreign keys.
--
-- PostgreSQL indexes the referenced side of a foreign key, never the
-- referencing side, so deleting a parent row scans the child table once per
-- constraint. Three of these point at document_versions, and version retention
-- deletes versions inline in the same transaction that commits a new one --
-- so each commit past the retention bound was scanning all of documents, all
-- of document_versions, and all of publications, on the write path.
CREATE INDEX documents_current_version ON documents(current_version_id)
    WHERE current_version_id IS NOT NULL;
CREATE INDEX documents_current_publication ON documents(current_publication_id)
    WHERE current_publication_id IS NOT NULL;
CREATE INDEX document_versions_parent ON document_versions(parent_id)
    WHERE parent_id IS NOT NULL;
CREATE INDEX publications_source_version ON publications(source_version_id)
    WHERE source_version_id IS NOT NULL;
CREATE INDEX jobs_document ON jobs(document_id) WHERE document_id IS NOT NULL;
CREATE INDEX jobs_account ON jobs(account_id) WHERE account_id IS NOT NULL;

-- 6. Terminal-job pruning.
--
-- Claiming and claim recovery each have their own partial index; the third
-- pass over this table, which deletes finished jobs oldest-first, had none and
-- sorted every terminal row to find a page of them.
CREATE INDEX jobs_terminal_cleanup ON jobs(updated_at, id)
    WHERE status IN ('succeeded', 'failed', 'cancelled');
