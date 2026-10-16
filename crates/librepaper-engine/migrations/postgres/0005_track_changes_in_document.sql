-- Tracked changes live in the Loro document, in its `changes` root map, so
-- the proposal tables and the suggestion annotations are gone. A suggestion
-- comment that was still open is deleted with its replies; its proposal
-- branch held the only copy of the proposed text and is dropped with it.
DELETE FROM annotations WHERE kind = 'suggestion';

ALTER TABLE annotations DROP CONSTRAINT annotations_document_proposal_fkey;
ALTER TABLE annotations DROP CONSTRAINT annotations_suggestion_proposal;
ALTER TABLE annotations DROP COLUMN proposal_id;

ALTER TABLE annotations DROP CONSTRAINT annotations_kind_check;
ALTER TABLE annotations
    ADD CONSTRAINT annotations_kind_check CHECK (kind IN ('comment','highlight'));

DROP TABLE document_proposal_outcomes;
DROP TABLE document_proposal_hunks;
DROP INDEX document_proposals_document;
DROP TABLE document_proposals;
