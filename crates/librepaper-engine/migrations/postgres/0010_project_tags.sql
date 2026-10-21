-- Project tags. A tag belongs to one account, so a collaborator on a shared
-- project keeps their own tags on it. Favorites is a built-in tag: its row
-- carries builtin = 'favorites' and is created the first time the account
-- stars something, so an account that never starred has no row at all.
CREATE TABLE tags (
    id uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name text NOT NULL,
    colour text NOT NULL,
    builtin text CHECK (builtin IN ('favorites')),
    created_at timestamptz NOT NULL DEFAULT now()
);
-- Names are unique per account ignoring case, so "Draft" and "draft" are one
-- tag. The built-in row takes the reserved name "Favorites" the same way.
CREATE UNIQUE INDEX tags_account_name ON tags(account_id, lower(name));
CREATE UNIQUE INDEX tags_account_builtin ON tags(account_id, builtin)
    WHERE builtin IS NOT NULL;

-- A tag on a document. Trashing a project soft deletes the document, so these
-- rows survive a trash and a restore. A purge removes the document and with
-- it these links.
CREATE TABLE document_tags (
    tag_id uuid NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    tagged_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tag_id, document_id)
);
CREATE INDEX document_tags_document ON document_tags(document_id);

-- Stars become links to the built-in tag, keeping the time each was starred
-- so "your favorites, most recent first" still has an order. gen_random_uuid()
-- is the only id source a migration has; the app mints UUIDv7 for every
-- other tag.
INSERT INTO tags (id, account_id, name, colour, builtin)
SELECT gen_random_uuid(), account_id, 'Favorites', 'yellow', 'favorites'
FROM (
    SELECT DISTINCT account_id FROM document_marks WHERE favorited_at IS NOT NULL
) starred;

INSERT INTO document_tags (tag_id, document_id, tagged_at)
SELECT t.id, m.document_id, m.favorited_at
FROM document_marks m
JOIN tags t ON t.account_id = m.account_id AND t.builtin = 'favorites'
WHERE m.favorited_at IS NOT NULL;

-- A mark that held only a star marks nothing once the star moves to tags.
DELETE FROM document_marks WHERE opened_at IS NULL AND NOT shared_hidden;

ALTER TABLE document_marks DROP CONSTRAINT document_marks_has_mark;
ALTER TABLE document_marks ADD CONSTRAINT document_marks_has_mark
    CHECK (opened_at IS NOT NULL OR shared_hidden);
ALTER TABLE document_marks DROP COLUMN favorited_at;
