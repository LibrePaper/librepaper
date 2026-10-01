-- Keep idempotency scoped to the log that owns the request. A client may
-- legitimately reuse one UUID in another document.
ALTER TABLE document_labels
    DROP CONSTRAINT document_labels_request_id_key;
ALTER TABLE document_labels
    ADD CONSTRAINT document_labels_document_request_id_key
    UNIQUE (document_id, request_id);

-- The validated check in 0003 already proves every stored annotation has
-- both values. Make that invariant visible to row types and query planning.
ALTER TABLE annotations
    ALTER COLUMN source_sequence SET NOT NULL,
    ALTER COLUMN frontier SET NOT NULL;

-- A suggestion belongs to a proposal in the same document. The proposal UUID
-- is globally unique today, but the composite key makes the tenant boundary
-- explicit and prevents mismatched-document references.
ALTER TABLE document_proposals
    ADD CONSTRAINT document_proposals_document_id_id_key UNIQUE (document_id, id);
ALTER TABLE annotations
    DROP CONSTRAINT annotations_proposal_id_fkey;
ALTER TABLE annotations
    ADD CONSTRAINT annotations_document_proposal_fkey
    FOREIGN KEY (document_id, proposal_id)
    REFERENCES document_proposals(document_id, id) ON DELETE CASCADE;

-- Provenance is authority: never turn a broken reference into a direct grant
-- during migration. Stop and report how many rows require explicit repair.
DO $$
DECLARE
    invalid_count bigint;
BEGIN
    SELECT count(*) INTO invalid_count
    FROM grants g
    WHERE g.source_link_hash IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM share_links l
          WHERE l.document_id = g.document_id
            AND l.token_hash = g.source_link_hash
      );

    IF invalid_count > 0 THEN
        RAISE EXCEPTION
            'cannot add grant provenance constraint: % grants reference no link in their document; repair provenance explicitly before retrying',
            invalid_count
            USING ERRCODE = '23503';
    END IF;
END $$;

ALTER TABLE share_links
    ADD CONSTRAINT share_links_document_token_hash_key UNIQUE (document_id, token_hash);
ALTER TABLE grants
    ADD CONSTRAINT grants_source_link_document_fkey
    FOREIGN KEY (document_id, source_link_hash)
    REFERENCES share_links(document_id, token_hash) ON DELETE CASCADE;

-- These columns have no live readers or writers. The snapshot check is
-- replaced first so legacy retired rows may keep their NULL evidence, while a
-- current snapshot continues to require its complete metadata.
ALTER TABLE document_snapshots
    DROP CONSTRAINT document_snapshots_current_metadata;
ALTER TABLE document_snapshots
    ADD CONSTRAINT document_snapshots_current_metadata CHECK (
        delete_after IS NOT NULL OR (
            through_update_sequence IS NOT NULL
            AND vector IS NOT NULL
            AND snapshot_digest IS NOT NULL
            AND snapshot_bytes IS NOT NULL
            AND updated_at IS NOT NULL
        )
    );
ALTER TABLE document_snapshots DROP COLUMN base_id;
ALTER TABLE share_links DROP COLUMN generation;
ALTER TABLE documents DROP COLUMN settings;
ALTER TABLE accounts DROP COLUMN preferences;
