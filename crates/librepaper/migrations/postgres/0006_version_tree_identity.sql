-- What a version holds, and what it moved.
--
-- A version's identity as a document is the digest of its canonical file
-- tree, not the digest of the archive that carries it: the archive is one
-- encoding of the tree and an encoder change would rename an unchanged
-- document. Without the tree digest on the row, a server that loaded the
-- timeline from here could not tell that the live document still says exactly
-- what the newest version says, and wrote a whole duplicate archive every
-- time a room was opened and closed.
ALTER TABLE document_versions ADD COLUMN tree_digest bytea
    CHECK (tree_digest IS NULL OR octet_length(tree_digest) = 32);

-- The paths whose contents differ from the parent version's, recorded once,
-- when the version is written and both trees are in hand. It is what lets the
-- history panel scope a timeline to the file being edited without opening two
-- trees for every row.
--
-- NULL means the question was never answered for this version -- every row
-- written before this column existed -- and readers must not mistake that for
-- an answer of "nothing".
ALTER TABLE document_versions ADD COLUMN changed_paths text[];
