-- Regression coverage for migrations/postgres/0004_archive_accounting.sql.
-- This is deliberately raw SQL so the Rust test can execute it with SQLx.
BEGIN;

CREATE TEMP TABLE archive_review_before ON COMMIT DROP AS
SELECT bytes FROM storage_usage WHERE singleton;

INSERT INTO accounts (id, kind, handle, display_name, status)
VALUES (
    '00000000-0000-0000-0000-000000000001', 'system',
    'archive-accounting-test', 'Archive accounting test', 'active'
);

INSERT INTO documents (id, slug, owner_id, ownership_mode, title, status,
                       source_format, main_path)
VALUES
    ('00000000-0000-0000-0000-000000000101', 'archive-accounting-source',
     '00000000-0000-0000-0000-000000000001', 'owned', 'Source', 'active',
     'markdown', 'README.md'),
    ('00000000-0000-0000-0000-000000000102', 'archive-accounting-survivor',
     '00000000-0000-0000-0000-000000000001', 'owned', 'Survivor', 'active',
     'markdown', 'README.md');

-- The surviving document's asset makes the shared-archive refund observable:
-- after deleting the source document, usage must be 2000, not 1500.
INSERT INTO document_assets (id, document_id, storage_key, digest, byte_length,
                             media_type)
VALUES (
    '00000000-0000-0000-0000-000000000301',
    '00000000-0000-0000-0000-000000000102',
    'assets/archive-accounting-survivor', decode(repeat('00', 32), 'hex'),
    2000, 'application/octet-stream'
);

-- Two labels in one document name one 500-byte archive.
INSERT INTO document_labels
    (id, document_id, sequence, source_sequence, vector, frontier, reason,
     author_label, archive_key, archive_bytes)
VALUES
    ('00000000-0000-0000-0000-000000000201',
     '00000000-0000-0000-0000-000000000101', 1, 1, decode('01', 'hex'),
     decode('11', 'hex'), 'restore', 'test', 'archives/shared-500', 500),
    ('00000000-0000-0000-0000-000000000202',
     '00000000-0000-0000-0000-000000000101', 2, 2, decode('02', 'hex'),
     decode('12', 'hex'), 'restore', 'test', 'archives/shared-500', 500);

DO $$
DECLARE
    actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2500 THEN
        RAISE EXCEPTION 'shared insert accounted % bytes, expected 2500', actual;
    END IF;
END
$$;

-- A document delete invokes the label DELETE trigger through a cascade.  The
-- two transition rows must refund the shared key once, leaving the survivor's
-- 2000-byte asset fully accounted for.
DELETE FROM documents
WHERE id = '00000000-0000-0000-0000-000000000101';

DO $$
DECLARE
    actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2000 THEN
        RAISE EXCEPTION 'cascade left % bytes, expected 2000', actual;
    END IF;
END
$$;

-- A multirow key switch removes one object and adds another exactly once.
INSERT INTO document_labels
    (id, document_id, sequence, source_sequence, vector, frontier, reason,
     author_label, archive_key, archive_bytes)
VALUES
    ('00000000-0000-0000-0000-000000000203',
     '00000000-0000-0000-0000-000000000102', 1, 1, decode('03', 'hex'),
     decode('13', 'hex'), 'restore', 'test', 'archives/old-700', 700),
    ('00000000-0000-0000-0000-000000000204',
     '00000000-0000-0000-0000-000000000102', 2, 2, decode('04', 'hex'),
     decode('14', 'hex'), 'restore', 'test', 'archives/old-700', 700);

UPDATE document_labels
SET archive_key = 'archives/new-800', archive_bytes = 800
WHERE id IN (
    '00000000-0000-0000-0000-000000000203',
    '00000000-0000-0000-0000-000000000204'
);

DO $$
DECLARE
    actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2800 THEN
        RAISE EXCEPTION 'multirow key switch left % bytes, expected 2800', actual;
    END IF;
END
$$;

DELETE FROM document_labels
WHERE id IN (
    '00000000-0000-0000-0000-000000000203',
    '00000000-0000-0000-0000-000000000204'
);

-- Updating archive_bytes for every row sharing a key changes that object once.
INSERT INTO document_labels
    (id, document_id, sequence, source_sequence, vector, frontier, reason,
     author_label, archive_key, archive_bytes)
VALUES
    ('00000000-0000-0000-0000-000000000205',
     '00000000-0000-0000-0000-000000000102', 3, 3, decode('05', 'hex'),
     decode('15', 'hex'), 'restore', 'test', 'archives/resized', 300),
    ('00000000-0000-0000-0000-000000000206',
     '00000000-0000-0000-0000-000000000102', 4, 4, decode('06', 'hex'),
     decode('16', 'hex'), 'restore', 'test', 'archives/resized', 300);

UPDATE document_labels
SET archive_bytes = 450
WHERE archive_key = 'archives/resized';

DO $$
DECLARE
    actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2450 THEN
        RAISE EXCEPTION 'same-key resize left % bytes, expected 2450', actual;
    END IF;
END
$$;

-- Adding a third reference is free, and resizing all three together still
-- moves one object.  Deleting only one reference is also free.
INSERT INTO document_labels
    (id, document_id, sequence, source_sequence, vector, frontier, reason,
     author_label, archive_key, archive_bytes)
VALUES (
    '00000000-0000-0000-0000-000000000207',
    '00000000-0000-0000-0000-000000000102', 5, 5, decode('07', 'hex'),
    decode('17', 'hex'), 'restore', 'test', 'archives/resized', 450
);

UPDATE document_labels
SET archive_bytes = 500
WHERE archive_key = 'archives/resized';

DELETE FROM document_labels
WHERE id = '00000000-0000-0000-0000-000000000205';

DO $$
DECLARE
    actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2500 THEN
        RAISE EXCEPTION 'shared delete left % bytes, expected 2500', actual;
    END IF;
END
$$;

DELETE FROM document_labels
WHERE archive_key = 'archives/resized';

DO $$
DECLARE
    actual bigint;
BEGIN
    SELECT bytes - (SELECT bytes FROM archive_review_before) INTO actual
    FROM storage_usage WHERE singleton;
    IF actual <> 2000 THEN
        RAISE EXCEPTION 'last archive delete left % bytes, expected 2000', actual;
    END IF;
END
$$;

ROLLBACK;
