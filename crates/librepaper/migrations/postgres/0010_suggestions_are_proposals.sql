-- A suggestion's text and its verdict move out of the comment.
--
-- A comment that suggested a change used to carry both: `proposed_text` said
-- what it wanted, and `suggestion_state` said whether an editor had taken it.
-- That made a comment a second implementation of "a change awaiting a
-- decision" -- the thing SPEC-loro.md §1.2 set out to stop there being four of.
--
-- Both now live where every proposed change lives. What the suggestion wants is
-- the difference its branch makes to the passage, and whether it was taken is a
-- row in `document_proposal_hunks`. The comment keeps the remark and carries
-- the proposal's id, which is enough to find either.
--
-- Dropping the columns takes with it the CHECK constraint that spanned them,
-- which is the point: the invariant it held -- a suggestion has proposed text,
-- anything else does not -- is not expressible here any more, and is not this
-- table's to hold.
ALTER TABLE annotations
    DROP COLUMN proposed_text,
    DROP COLUMN suggestion_state;
