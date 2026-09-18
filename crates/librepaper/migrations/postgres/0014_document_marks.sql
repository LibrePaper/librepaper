-- What one person has done with one document, as distinct from what they may
-- do to it.
--
-- `grants` already says what a person is allowed to do with a document, and it
-- exists only for documents somebody was given. Favouriting and opening are
-- neither permissions nor gifts: you favourite your own work, and the document
-- you opened most recently is usually one nobody had to share with you. So
-- this is its own table rather than more columns on `grants`, which would have
-- to grow a row with no role in it for every project its owner starred.
--
-- Both facts were kept in the browser until now -- `FAVORITES` and `VIEWED` in
-- web/src/lib/storage.js -- which made a favourite a property of a laptop. A
-- person with a desktop and a laptop had two sets of favourites and neither
-- was wrong. The listing is about to offer Favorites and Recent as places to
-- go rather than as a checkbox over a table, and a place that is empty on the
-- machine you are not sitting at is not a place. So they move here, and the
-- browser keys are deleted rather than migrated: nothing is released, and a
-- one-way import from whichever machine happened to sign in first would settle
-- the question wrongly for everyone who has two.
--
-- One table for both, because they are the same shape -- a person, a document,
-- a timestamp -- and a listing that shows Favorites sorted by when you last
-- opened them would otherwise join the same pair of ids twice.
CREATE TABLE document_marks (
    account_id uuid NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    -- When it was favourited, not whether: "your favourites, most recent
    -- first" is the order a list of them wants, and a boolean cannot answer
    -- it. NULL is a document this person has opened but not starred.
    favorited_at timestamptz,
    -- The last time this person opened it. NULL is a document they have
    -- starred without opening -- rare, but reachable: a project shared with
    -- you can be starred from the listing before you ever go in.
    opened_at timestamptz,
    PRIMARY KEY (account_id, document_id),
    -- A row that says neither is a row that says nothing. Un-favouriting
    -- something never opened deletes the row rather than blanking the column,
    -- so the table holds marks and not the debris of marks.
    CHECK (favorited_at IS NOT NULL OR opened_at IS NOT NULL)
);

-- One index per destination in the listing's rail, both partial: a person's
-- recently-opened projects are a small suffix of everything they have ever
-- opened, and their favourites a small subset of that. Ordering the index the
-- way the query reads it means Recent and Favorites are an index scan of the
-- first page and nothing else, however many projects the account has.
CREATE INDEX document_marks_favorites ON document_marks(account_id, favorited_at DESC)
    WHERE favorited_at IS NOT NULL;
CREATE INDEX document_marks_recent ON document_marks(account_id, opened_at DESC)
    WHERE opened_at IS NOT NULL;

-- Deleting a document deletes its marks, by the cascade above. That is the
-- right rule for a purge and the wrong one for the seven-day recovery window
-- that precedes it -- but the window does not delete the row, it only sets
-- `documents.status` to 'deleting', so a project restored from the trash comes
-- back still starred. The cascade fires when the deletion job finally removes
-- the row, by which time there is nothing left to be a favourite of.
