-- The author of a comment or reply may change its body. No history is kept;
-- the row only says when it was last edited, and is null until the first edit.
ALTER TABLE annotations ADD COLUMN edited_at timestamptz;
ALTER TABLE replies ADD COLUMN edited_at timestamptz;
