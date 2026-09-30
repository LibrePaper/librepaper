-- Regression coverage for normalized, retained document archive objects.
BEGIN;

CREATE TEMP TABLE archive_review_before ON COMMIT DROP AS
SELECT bytes FROM storage_usage WHERE singleton;

INSERT INTO accounts (id, kind, handle, display_name, status)
VALUES ('00000000-0000-0000-0000-000000000001', 'system',
        'archive-accounting-test', 'Archive accounting test', 'active');

INSERT INTO documents (id, slug, owner_id, ownership_mode, title, status,
                       source_format, main_path)
VALUES ('00000000-0000-0000-0000-000000000101', 'archive-accounting-source',
        '00000000-0000-0000-0000-000000000001', 'owned', 'Source', 'active',
        'markdown', 'README.md'),
       ('00000000-0000-0000-0000-000000000102', 'archive-accounting-survivor',
        '00000000-0000-0000-0000-000000000001', 'owned', 'Survivor', 'active',
        'markdown', 'README.md');

INSERT INTO document_assets (id, document_id, storage_key, digest, byte_length,
                             media_type)
VALUES ('00000000-0000-0000-0000-000000000301',
        '00000000-0000-0000-0000-000000000102',
        'assets/archive-accounting-survivor', decode(repeat('00', 32), 'hex'),
        2000, 'application/octet-stream');

-- Legacy object metadata is intentionally nullable. Both labels point at one
-- retained object, and the backfilled key remains intact.
INSERT INTO document_archives (storage_key, document_id, byte_length)
VALUES ('archives/shared-500', '00000000-0000-0000-0000-000000000101', 500);
INSERT INTO document_labels
    (id, document_id, sequence, source_sequence, vector, frontier, reason,
     author_label, archive_key)
VALUES ('00000000-0000-0000-0000-000000000201',
        '00000000-0000-0000-0000-000000000101', 1, 1, decode('01', 'hex'),
        decode('11', 'hex'), 'restore', 'test', 'archives/shared-500'),
       ('00000000-0000-0000-0000-000000000202',
        '00000000-0000-0000-0000-000000000101', 2, 2, decode('02', 'hex'),
        decode('12', 'hex'), 'restore', 'test', 'archives/shared-500');

DO $$
DECLARE actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2500 THEN
        RAISE EXCEPTION 'shared archive accounted % bytes, expected 2500', actual;
    END IF;
END
$$;

-- Removing labels does not free the object. It remains addressable for the
-- document's lifetime and is included in backups and owner storage totals.
DELETE FROM document_labels
WHERE id IN ('00000000-0000-0000-0000-000000000201',
             '00000000-0000-0000-0000-000000000202');
DO $$
DECLARE actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2500 THEN
        RAISE EXCEPTION 'unreferenced archive was not retained: % bytes', actual;
    END IF;
END
$$;

-- A new immutable object gets one row and is charged once. Document deletion
-- cascades both archive rows and refunds their bytes exactly once.
INSERT INTO document_archives
    (storage_key, document_id, tree_digest, content_digest, byte_length)
VALUES ('archives/new-800', '00000000-0000-0000-0000-000000000101',
        decode(repeat('11', 32), 'hex'), decode(repeat('22', 32), 'hex'), 800);
DO $$
DECLARE actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 3300 THEN
        RAISE EXCEPTION 'new archive accounted % bytes, expected 3300', actual;
    END IF;
END
$$;

DELETE FROM documents WHERE id = '00000000-0000-0000-0000-000000000101';
DO $$
DECLARE actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2000 THEN
        RAISE EXCEPTION 'document cascade left % bytes, expected 2000', actual;
    END IF;
END
$$;

ROLLBACK;
