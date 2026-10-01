-- A proposal is deleted when it is decided or discarded. Keep bounded,
-- expiring evidence so a reconnecting author can separate its reviewed branch
-- from edits that were still local when the final answer was lost.
CREATE TABLE document_proposal_outcomes (
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    proposal_id uuid NOT NULL,
    base_frontiers bytea NOT NULL,
    tip_frontiers bytea NOT NULL,
    decisions jsonb NOT NULL DEFAULT '[]'::jsonb,
    discarded boolean NOT NULL DEFAULT false,
    decision_request_id uuid,
    expires_at bigint NOT NULL,
    PRIMARY KEY (document_id, proposal_id)
);

CREATE INDEX document_proposal_outcomes_expiry
    ON document_proposal_outcomes(document_id, expires_at);
