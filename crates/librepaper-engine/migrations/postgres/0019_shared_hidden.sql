ALTER TABLE document_marks
    ADD COLUMN shared_hidden boolean NOT NULL DEFAULT false;

ALTER TABLE document_marks
    DROP CONSTRAINT document_marks_check,
    ADD CONSTRAINT document_marks_has_mark
        CHECK (favorited_at IS NOT NULL OR opened_at IS NOT NULL OR shared_hidden);
