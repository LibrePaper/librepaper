-- Every proposal row is pending.
--
-- A proposal is now deleted when its last hunk is decided (see
-- room/proposals.rs), so the rows that remain are pending by construction and
-- status, resolved_by and resolved_at carry nothing.
--
-- Rows left by the old behaviour go first: everything that is not pending, and
-- every pending proposal created before 2026-09-28T19:36:52Z. Until that
-- moment the browser could not decide a proposal at all because of a request
-- id bug, so those are abandoned. Deleting a proposal cascades to its hunks and
-- its suggestion annotation.

DELETE FROM document_proposals
WHERE status <> 'pending' OR created_at < '2026-09-28T19:36:52Z';

DROP INDEX document_proposals_open;

ALTER TABLE document_proposals
    DROP COLUMN status,
    DROP COLUMN resolved_by,
    DROP COLUMN resolved_at;

CREATE INDEX document_proposals_document ON document_proposals(document_id);
