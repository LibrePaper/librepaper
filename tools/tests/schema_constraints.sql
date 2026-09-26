-- Behaviour regression assertions for postgres migration 0003.
-- Run against a database after applying all catalogue migrations. The whole
-- script rolls back, so it leaves no catalogue rows behind.

BEGIN;

INSERT INTO accounts (id, kind, handle, display_name, status)
VALUES ('00000000-0000-0000-0000-000000000001', 'anonymous',
        'schema-test', 'Schema test', 'active');

INSERT INTO documents (id, slug, owner_id, ownership_mode, title, status,
                       source_format, main_path)
VALUES ('00000000-0000-0000-0000-000000000002', 'schema-constraints-test',
        '00000000-0000-0000-0000-000000000001', 'owned',
        'Schema constraints test', 'active', 'markdown', 'main.md');

-- Both target forms remain valid when their required evidence is present.
INSERT INTO annotations (
    id, document_id, kind, body, author_account_id, author_key, author_label,
    source_sequence, frontier, target_kind, file_id, start_utf16, end_utf16,
    start_side, end_side, exact, prefix, suffix
) VALUES (
    '00000000-0000-0000-0000-000000000010',
    '00000000-0000-0000-0000-000000000002', 'comment', 'source text',
    '00000000-0000-0000-0000-000000000001', 'schema-test', 'Schema test',
    1, decode('00', 'hex'), 'source_text', 'file-1', 0, 1,
    'left', 'right', 'x', '', ''
);

INSERT INTO annotations (
    id, document_id, kind, body, author_account_id, author_key, author_label,
    source_sequence, frontier, target_kind
) VALUES (
    '00000000-0000-0000-0000-000000000011',
    '00000000-0000-0000-0000-000000000002', 'comment', 'document',
    '00000000-0000-0000-0000-000000000001', 'schema-test', 'Schema test',
    1, decode('00', 'hex'), 'document'
);

-- Every malformed source-text annotation must fail its CHECK constraint.
DO $$
BEGIN
    BEGIN
        INSERT INTO annotations (
            id, document_id, kind, body, author_account_id, author_key, author_label,
            source_sequence, frontier, target_kind, file_id, start_utf16, end_utf16,
            end_side, exact, prefix, suffix
        ) VALUES (
            '00000000-0000-0000-0000-000000000020',
            '00000000-0000-0000-0000-000000000002', 'comment', 'missing start side',
            '00000000-0000-0000-0000-000000000001', 'schema-test', 'Schema test',
            1, decode('00', 'hex'), 'source_text', 'file-1', 0, 1,
            'right', 'x', '', ''
        );
        RAISE EXCEPTION 'source-text annotation with NULL start_side was accepted';
    EXCEPTION WHEN check_violation THEN NULL;
    END;

    BEGIN
        INSERT INTO annotations (
            id, document_id, kind, body, author_account_id, author_key, author_label,
            source_sequence, frontier, target_kind, file_id, start_utf16, end_utf16,
            start_side, exact, prefix, suffix
        ) VALUES (
            '00000000-0000-0000-0000-000000000021',
            '00000000-0000-0000-0000-000000000002', 'comment', 'missing end side',
            '00000000-0000-0000-0000-000000000001', 'schema-test', 'Schema test',
            1, decode('00', 'hex'), 'source_text', 'file-1', 0, 1,
            'left', 'x', '', ''
        );
        RAISE EXCEPTION 'source-text annotation with NULL end_side was accepted';
    EXCEPTION WHEN check_violation THEN NULL;
    END;

    BEGIN
        INSERT INTO annotations (
            id, document_id, kind, body, author_account_id, author_key, author_label,
            source_sequence, frontier, target_kind, file_id, start_utf16, end_utf16,
            start_side, end_side, exact, prefix, suffix
        ) VALUES (
            '00000000-0000-0000-0000-000000000022',
            '00000000-0000-0000-0000-000000000002', 'comment', 'backwards range',
            '00000000-0000-0000-0000-000000000001', 'schema-test', 'Schema test',
            1, decode('00', 'hex'), 'source_text', 'file-1', 1, 0,
            'left', 'right', 'x', '', ''
        );
        RAISE EXCEPTION 'source-text annotation with a backwards range was accepted';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
END;
$$;

-- Both evidence columns are mandatory for every annotation, including a
-- document-level annotation.
DO $$
BEGIN
    BEGIN
        INSERT INTO annotations (
            id, document_id, kind, body, author_account_id, author_key, author_label,
            frontier, target_kind
        ) VALUES (
            '00000000-0000-0000-0000-000000000023',
            '00000000-0000-0000-0000-000000000002', 'comment', 'missing sequence',
            '00000000-0000-0000-0000-000000000001', 'schema-test', 'Schema test',
            decode('00', 'hex'), 'document'
        );
        RAISE EXCEPTION 'annotation with NULL source_sequence was accepted';
    EXCEPTION WHEN check_violation THEN NULL;
    END;

    BEGIN
        INSERT INTO annotations (
            id, document_id, kind, body, author_account_id, author_key, author_label,
            source_sequence, target_kind
        ) VALUES (
            '00000000-0000-0000-0000-000000000024',
            '00000000-0000-0000-0000-000000000002', 'comment', 'missing frontier',
            '00000000-0000-0000-0000-000000000001', 'schema-test', 'Schema test',
            1, 'document'
        );
        RAISE EXCEPTION 'annotation with NULL frontier was accepted';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
END;
$$;

ROLLBACK;
