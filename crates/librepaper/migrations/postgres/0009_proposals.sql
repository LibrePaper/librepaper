-- A proposed change is a branch, and its status lives here rather than in the
-- document.
--
-- What "a change awaiting accept or reject" used to be was a JSON record with
-- encoded anchors inside the shared CRDT, which is why the server had to police
-- field-level mutations on every inbound update: a client that can write to the
-- document can write to anything stored in it, including the record that says
-- whether its own edit was approved. Keeping the decision here instead is what
-- makes it server-owned, and it is what let the rehearsal on the admission path
-- be deleted rather than optimised.
--
-- The branch itself is stored here too, as the operations since its fork. It
-- could have gone to the blob store alongside the document bases, and symmetry
-- argued for that, but a base is large, long-lived and read on cold start while
-- a branch is none of those. More decisively, accepting a proposal has to write
-- the decision and the resulting document update together or not at all, and one
-- transaction over one store is how that is actually achieved.
CREATE TABLE document_proposals (
    id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,

    -- Who opened it, for attribution in the UI. The authorship that matters for
    -- contract 4 is not this: it is carried by the operations themselves, and
    -- survives a partial accept because the branch is merged whole.
    author text NOT NULL,

    -- The peer that wrote the branch. Kept so the room can tell a proposal's own
    -- operations from everyone else's without decoding the blob.
    author_peer bigint NOT NULL,

    -- Encoded frontiers. `base` is where the branch forked; `tip` is where it
    -- had reached when it was last updated. Review is the diff between them, and
    -- a decision names the tip it was computed against so that a decision made
    -- against a stale tip can be refused rather than applied to text that has
    -- since changed underneath the reviewer.
    base_frontiers bytea NOT NULL,
    tip_frontiers bytea NOT NULL,

    -- The branch: Loro operations since the fork, not a whole document.
    branch_bytes bytea NOT NULL,

    -- `pending` until every hunk has been decided; then `resolved`. A proposal
    -- that the document has moved past without deciding is `superseded`.
    -- Whether the change landed is a property of its hunks, not of the proposal
    -- -- accepting half of one is the ordinary case, so there is no single
    -- accepted/declined answer to record here.
    status text NOT NULL CHECK (status IN ('pending', 'resolved', 'superseded')),

    -- Set when the proposal resolves, together in one write with the update that
    -- carried its accepted hunks into the document.
    resolved_by text,
    resolved_at timestamptz,

    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- The open proposals for a document, which is what a joining client asks for and
-- what the room broadcasts after every decision.
CREATE INDEX document_proposals_open ON document_proposals(document_id, status);

-- One row per decision, because a reviewer decides hunks and not proposals.
--
-- A hunk is named by its index in the diff of a named tip against a named base
-- (§5.2). That identity is deliberately free of any offset: the server counts
-- hunks in code points and the browser counts them in UTF-16, so an index is the
-- one way to name a hunk that means the same thing on both sides. It is stable
-- only against the tip it was computed from, which is why that tip is recorded
-- beside it.
CREATE TABLE document_proposal_hunks (
    proposal_id uuid NOT NULL REFERENCES document_proposals(id) ON DELETE CASCADE,
    hunk_index integer NOT NULL CHECK (hunk_index >= 0),
    accepted boolean NOT NULL,
    decided_by text NOT NULL,
    decided_at timestamptz NOT NULL DEFAULT now(),

    -- The tip this decision was computed against. A decision whose tip is no
    -- longer the proposal's tip was made about a diff that no longer exists.
    decided_against bytea NOT NULL,

    -- A reviewer may say why, and declining is where that matters most.
    note text,

    PRIMARY KEY (proposal_id, hunk_index)
);
