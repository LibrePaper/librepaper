-- A comment stops being a quotation and becomes a place in the source.
--
-- What a comment is about used to be a `selector` of rendered words, with an
-- optional source anchor beside it in `context` that some clients filled in
-- and some did not. Two answers to one question, either of which could be
-- believed, and a path in the second of them -- so renaming a file quietly
-- changed what a comment was about.
--
-- There is one answer now, and it is a range: the file (by its stable Loro id,
-- never its name), the UTF-16 offsets in it, and the checkpoint those offsets
-- are offsets into. It is written once, by the server, from the words a client
-- sent, and no later event edits it. Where that passage has got to since is a
-- separate row in `annotation_live_state`, which is a cache: deleting all of
-- it costs one recomputation and loses nothing.
--
-- The rendered words stay, in their own columns, under names that say what
-- they are. They are what a comment looked like where it was made -- worth
-- showing a person, never resolved through.

ALTER TABLE annotations
    ADD COLUMN checkpoint_id text,
    ADD COLUMN target_kind text,
    ADD COLUMN file_id text,
    ADD COLUMN start_utf16 integer,
    ADD COLUMN end_utf16 integer,
    ADD COLUMN start_side text,
    ADD COLUMN end_side text,
    ADD COLUMN exact text,
    ADD COLUMN prefix text,
    ADD COLUMN suffix text,
    ADD COLUMN rendered_exact text NOT NULL DEFAULT '',
    ADD COLUMN rendered_prefix text NOT NULL DEFAULT '',
    ADD COLUMN rendered_suffix text NOT NULL DEFAULT '',
    ADD COLUMN rendered_position_utf16 integer,
    ADD COLUMN color text;

-- Existing annotations predate durable source identity: the rendered quote is
-- all most of them have, and the source anchors that do exist name a path
-- rather than a file. Turning either into a range would mean guessing which
-- file and which occurrence, in SQL, without the document -- so they are
-- retired here instead, as SPEC-loro.md's comment redesign says to. This is a
-- development-stage catalogue; there is no deployment to preserve.
DELETE FROM replies WHERE annotation_id IN (SELECT id FROM annotations);
DELETE FROM annotations;

ALTER TABLE annotations
    ALTER COLUMN checkpoint_id SET NOT NULL,
    ALTER COLUMN target_kind SET NOT NULL,
    ADD CONSTRAINT annotations_target_kind CHECK (target_kind IN ('source_text', 'document')),
    -- A passage carries a range and its evidence; a remark about the document
    -- as a whole carries none of it. Neither can be half filled in.
    ADD CONSTRAINT annotations_source_text_target CHECK (
        (target_kind = 'source_text'
            AND file_id IS NOT NULL
            AND start_utf16 IS NOT NULL AND start_utf16 >= 0
            AND end_utf16 IS NOT NULL AND end_utf16 >= start_utf16
            AND start_side IN ('left', 'right')
            AND end_side IN ('left', 'right')
            AND exact IS NOT NULL AND prefix IS NOT NULL AND suffix IS NOT NULL)
        OR
        (target_kind = 'document'
            AND file_id IS NULL
            AND start_utf16 IS NULL AND end_utf16 IS NULL
            AND start_side IS NULL AND end_side IS NULL
            AND exact IS NULL AND prefix IS NULL AND suffix IS NULL)
    );

ALTER TABLE annotations
    DROP COLUMN selector,
    DROP COLUMN context,
    DROP COLUMN source_version_id,
    DROP COLUMN source_update_sequence,
    DROP COLUMN source_project_generation,
    DROP COLUMN source_state_vector;

-- Where each comment's passage is now, as last resolved. Every column here is
-- replaceable: the row is keyed by the checkpoint it was computed against and
-- is rewritten whenever the document moves. Nothing reads it to find out what
-- a comment is about.
--
-- The cursors are Loro's own anchors, captured when the comment was made and
-- replaced by whatever Loro hands back on a later resolution. They live here,
-- and not beside the range above, for exactly that reason.
CREATE TABLE annotation_live_state (
    annotation_id uuid PRIMARY KEY REFERENCES annotations(id) ON DELETE CASCADE,
    checkpoint_id text,
    status text NOT NULL DEFAULT 'unresolved'
        CHECK (status IN ('exact','modified','ambiguous','deleted','unresolved')),
    start_cursor bytea,
    end_cursor bytea,
    cursor_format text,
    resolved_start_utf16 integer,
    resolved_end_utf16 integer,
    diagnostic text,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((start_cursor IS NULL) = (end_cursor IS NULL)),
    CHECK ((start_cursor IS NULL) OR cursor_format IS NOT NULL)
);

-- Finding every comment on a file, which is what a resolution pass walks.
CREATE INDEX annotations_source_file ON annotations(document_id, file_id, created_at, id)
    WHERE file_id IS NOT NULL;
