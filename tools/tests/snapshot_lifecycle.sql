-- Behaviour regression assertions for the document_snapshots catalogue table.
-- Run against a database after applying all catalogue migrations.  The whole
-- script rolls back, so it leaves no catalogue rows behind.

BEGIN;

INSERT INTO accounts (id, kind, handle, display_name, status)
VALUES ('00000000-0000-0000-0000-000000000001', 'anonymous',
        'snapshot-lifecycle-test', 'Snapshot lifecycle test', 'active');

INSERT INTO documents (id, slug, owner_id, ownership_mode, title, status,
                       source_format, main_path)
VALUES ('00000000-0000-0000-0000-000000000002', 'snapshot-lifecycle-test',
        '00000000-0000-0000-0000-000000000001', 'owned',
        'Snapshot lifecycle test', 'active', 'markdown', 'main.md');

-- A current snapshot requires the complete snapshot metadata.
INSERT INTO document_snapshots (
    snapshot_key, document_id, base_id, through_update_sequence, vector,
    snapshot_digest, snapshot_bytes, updated_at, delete_after
) VALUES (
    'snapshots/lifecycle/current-1',
    '00000000-0000-0000-0000-000000000002',
    '00000000-0000-0000-0000-000000000011', 7, decode('0102', 'hex'),
    decode(repeat('ab', 32), 'hex'), 1234, '2026-09-20 12:00:00+00', NULL
);

-- The partial unique index permits only one current snapshot per document.
DO $$
BEGIN
    BEGIN
        INSERT INTO document_snapshots (
            snapshot_key, document_id, base_id, through_update_sequence, vector,
            snapshot_digest, snapshot_bytes, updated_at, delete_after
        ) VALUES (
            'snapshots/lifecycle/current-2',
            '00000000-0000-0000-0000-000000000002',
            '00000000-0000-0000-0000-000000000012', 8, decode('0304', 'hex'),
            decode(repeat('cd', 32), 'hex'), 2345,
            '2026-09-20 12:01:00+00', NULL
        );
        RAISE EXCEPTION 'two current snapshots for one document were accepted';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
END;
$$;

DO $$
BEGIN
    BEGIN
        INSERT INTO document_snapshots (
            snapshot_key, document_id, delete_after
        ) VALUES (
            'snapshots/lifecycle/current-incomplete',
            '00000000-0000-0000-0000-000000000002', NULL
        );
        RAISE EXCEPTION 'current snapshot with missing metadata was accepted';
    EXCEPTION WHEN check_violation THEN NULL;
    END;
END;
$$;

-- Retired rows retain the key and document but may lack legacy metadata.
INSERT INTO document_snapshots (snapshot_key, document_id, delete_after)
VALUES
    ('snapshots/lifecycle/legacy-1',
     '00000000-0000-0000-0000-000000000002',
     '2026-09-19 00:00:00+00'),
    ('snapshots/lifecycle/legacy-2',
     '00000000-0000-0000-0000-000000000002',
     '2026-09-19 01:00:00+00');

-- Retiring a current row preserves its metadata, and a replacement current
-- row can be inserted in the same transaction.
UPDATE document_snapshots
SET delete_after = '2026-09-20 12:02:00+00'
WHERE snapshot_key = 'snapshots/lifecycle/current-1';

DO $$
DECLARE
    stored_base uuid;
    stored_sequence bigint;
    stored_vector bytea;
    stored_digest bytea;
    stored_bytes bigint;
    stored_updated_at timestamptz;
BEGIN
    SELECT base_id, through_update_sequence, vector, snapshot_digest,
           snapshot_bytes, updated_at
      INTO stored_base, stored_sequence, stored_vector, stored_digest,
           stored_bytes, stored_updated_at
      FROM document_snapshots
     WHERE snapshot_key = 'snapshots/lifecycle/current-1';
    IF stored_base IS DISTINCT FROM '00000000-0000-0000-0000-000000000011'
       OR stored_sequence IS DISTINCT FROM 7
       OR stored_vector IS DISTINCT FROM decode('0102', 'hex')
       OR stored_digest IS DISTINCT FROM decode(repeat('ab', 32), 'hex')
       OR stored_bytes IS DISTINCT FROM 1234
       OR stored_updated_at IS DISTINCT FROM '2026-09-20 12:00:00+00'::timestamptz
       OR NOT EXISTS (
           SELECT 1 FROM document_snapshots
            WHERE snapshot_key = 'snapshots/lifecycle/current-1'
              AND delete_after = '2026-09-20 12:02:00+00'
       ) THEN
        RAISE EXCEPTION 'retiring a snapshot changed its stored metadata';
    END IF;
END;
$$;

INSERT INTO document_snapshots (
    snapshot_key, document_id, base_id, through_update_sequence, vector,
    snapshot_digest, snapshot_bytes, updated_at, delete_after
) VALUES (
    'snapshots/lifecycle/current-2',
    '00000000-0000-0000-0000-000000000002',
    '00000000-0000-0000-0000-000000000012', 8, decode('0304', 'hex'),
    decode(repeat('cd', 32), 'hex'), 2345, '2026-09-20 12:03:00+00', NULL
);

-- Deletion only removes expired retired snapshots.  The current snapshot and
-- a retired snapshot still inside its grace period must survive.
INSERT INTO document_snapshots (snapshot_key, document_id, delete_after)
VALUES
    ('snapshots/lifecycle/expired',
     '00000000-0000-0000-0000-000000000002', now() - interval '1 hour'),
    ('snapshots/lifecycle/future-grace',
     '00000000-0000-0000-0000-000000000002', now() + interval '1 hour');

DELETE FROM document_snapshots
 WHERE document_id='00000000-0000-0000-0000-000000000002'
   AND delete_after <= now();

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM document_snapshots
         WHERE snapshot_key = 'snapshots/lifecycle/expired'
    ) THEN
        RAISE EXCEPTION 'expired snapshot was not deleted';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM document_snapshots
         WHERE snapshot_key = 'snapshots/lifecycle/current-2'
           AND delete_after IS NULL
    ) THEN
        RAISE EXCEPTION 'current snapshot was deleted by expiry sweep';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM document_snapshots
         WHERE snapshot_key = 'snapshots/lifecycle/future-grace'
    ) THEN
        RAISE EXCEPTION 'future-grace snapshot was deleted too early';
    END IF;
END;
$$;

-- Document deletion cascades to both current and retired snapshots.
DELETE FROM documents
 WHERE id = '00000000-0000-0000-0000-000000000002';

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM document_snapshots
         WHERE document_id = '00000000-0000-0000-0000-000000000002'
    ) THEN
        RAISE EXCEPTION 'document deletion left snapshot rows behind';
    END IF;
END;
$$;

ROLLBACK;
