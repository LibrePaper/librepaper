-- Account archive objects once per statement, including labels removed by a
-- cascading document delete.  The old row trigger asked whether a key still
-- had siblings once per deleted row; a cascade can make every row observe the
-- same post-delete table and refund one shared object repeatedly.

-- Freeze both sources of the counter before changing triggers or reconciling
-- existing drift. The migration releases these locks when it commits.
LOCK TABLE document_assets, document_labels IN ACCESS EXCLUSIVE MODE;

DROP TRIGGER IF EXISTS document_labels_storage_usage ON document_labels;
DROP FUNCTION IF EXISTS adjust_label_archive_storage_usage();

-- The storage row is the accounting mutex.  Taking it before reading the
-- post-statement table serializes deltas from concurrent archive statements;
-- each waiter then gets a fresh READ COMMITTED snapshot that includes the
-- transaction whose delta it follows.  A negative result is allowed to fail
-- the CHECK on storage_usage.bytes: clamping would hide existing drift.

CREATE FUNCTION adjust_label_archive_storage_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    delta bigint;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM new_rows WHERE archive_key IS NOT NULL
    ) THEN
        RETURN NULL;
    END IF;

    PERFORM 1 FROM storage_usage WHERE singleton FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'storage_usage singleton is missing';
    END IF;

    -- archive_key names one object, so every row naming a key must agree on
    -- its byte length.  The pair check below applies to each row; this check
    -- keeps a same-key byte update from silently changing that object's size.
    IF EXISTS (
        SELECT 1
        FROM document_labels
        WHERE archive_key IN (SELECT archive_key FROM new_rows WHERE archive_key IS NOT NULL)
        GROUP BY archive_key
        HAVING min(archive_bytes) IS DISTINCT FROM max(archive_bytes)
    ) THEN
        RAISE EXCEPTION 'labels sharing an archive key must agree on archive_bytes';
    END IF;

    WITH affected AS (
             SELECT DISTINCT archive_key
             FROM new_rows
             WHERE archive_key IS NOT NULL
         ),
         new_usage AS (
             SELECT l.archive_key, max(l.archive_bytes) AS bytes
             FROM document_labels l
             JOIN affected a USING (archive_key)
             GROUP BY l.archive_key
         ),
         old_usage AS (
             SELECT l.archive_key, max(l.archive_bytes) AS bytes
             FROM document_labels l
             JOIN affected a USING (archive_key)
             WHERE NOT EXISTS (SELECT 1 FROM new_rows n WHERE n.id = l.id)
             GROUP BY l.archive_key
         )
    SELECT COALESCE(sum(COALESCE(n.bytes, 0) - COALESCE(o.bytes, 0)), 0)
      INTO delta
      FROM affected a
      LEFT JOIN new_usage n USING (archive_key)
      LEFT JOIN old_usage o USING (archive_key);

    UPDATE storage_usage SET bytes = bytes + delta WHERE singleton;
    RETURN NULL;
END
$$;

CREATE FUNCTION adjust_label_archive_storage_delete() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    delta bigint;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM old_rows WHERE archive_key IS NOT NULL
    ) THEN
        RETURN NULL;
    END IF;

    PERFORM 1 FROM storage_usage WHERE singleton FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'storage_usage singleton is missing';
    END IF;

    WITH affected AS (
             SELECT DISTINCT archive_key
             FROM old_rows
             WHERE archive_key IS NOT NULL
         ),
         new_usage AS (
             SELECT l.archive_key, max(l.archive_bytes) AS bytes
             FROM document_labels l
             JOIN affected a USING (archive_key)
             GROUP BY l.archive_key
         ),
         old_usage AS (
             SELECT archive_key, max(archive_bytes) AS bytes
             FROM (
                 SELECT l.archive_key, l.archive_bytes
                 FROM document_labels l
                 JOIN affected a USING (archive_key)
                 UNION ALL
                 SELECT o.archive_key, o.archive_bytes
                 FROM old_rows o
                 WHERE o.archive_key IS NOT NULL
             ) rows_before
             GROUP BY archive_key
         )
    SELECT COALESCE(sum(COALESCE(n.bytes, 0) - COALESCE(o.bytes, 0)), 0)
      INTO delta
      FROM affected a
      LEFT JOIN new_usage n USING (archive_key)
      LEFT JOIN old_usage o USING (archive_key);

    UPDATE storage_usage SET bytes = bytes + delta WHERE singleton;
    RETURN NULL;
END
$$;

CREATE FUNCTION adjust_label_archive_storage_update() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    delta bigint;
BEGIN
    -- Transition tables are only available on a statement trigger, so this
    -- trigger fires for every UPDATE and filters unrelated column changes
    -- here.  It also avoids taking the accounting mutex for a no-op archive
    -- update (including rows whose key and weight are both unchanged).
    IF NOT EXISTS (
        SELECT 1
        FROM old_rows o
        FULL JOIN new_rows n USING (id)
        WHERE o.id IS NULL OR n.id IS NULL
           OR o.archive_key IS DISTINCT FROM n.archive_key
           OR o.archive_bytes IS DISTINCT FROM n.archive_bytes
    ) THEN
        RETURN NULL;
    END IF;

    PERFORM 1 FROM storage_usage WHERE singleton FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'storage_usage singleton is missing';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM document_labels
        WHERE archive_key IN (
            SELECT archive_key FROM old_rows WHERE archive_key IS NOT NULL
            UNION
            SELECT archive_key FROM new_rows WHERE archive_key IS NOT NULL
        )
        GROUP BY archive_key
        HAVING min(archive_bytes) IS DISTINCT FROM max(archive_bytes)
    ) THEN
        RAISE EXCEPTION 'labels sharing an archive key must agree on archive_bytes';
    END IF;

    WITH affected AS (
             SELECT archive_key
             FROM old_rows
             WHERE archive_key IS NOT NULL
             UNION
             SELECT archive_key
             FROM new_rows
             WHERE archive_key IS NOT NULL
         ),
         new_usage AS (
             SELECT l.archive_key, max(l.archive_bytes) AS bytes
             FROM document_labels l
             JOIN affected a USING (archive_key)
             GROUP BY l.archive_key
         ),
         old_usage AS (
             SELECT archive_key, max(archive_bytes) AS bytes
             FROM (
                 SELECT l.archive_key, l.archive_bytes
                 FROM document_labels l
                 JOIN affected a USING (archive_key)
                 WHERE NOT EXISTS (SELECT 1 FROM new_rows n WHERE n.id = l.id)
                 UNION ALL
                 SELECT o.archive_key, o.archive_bytes
                 FROM old_rows o
                 WHERE o.archive_key IS NOT NULL
             ) rows_before
             GROUP BY archive_key
         )
    SELECT COALESCE(sum(COALESCE(n.bytes, 0) - COALESCE(o.bytes, 0)), 0)
      INTO delta
      FROM affected a
      LEFT JOIN new_usage n USING (archive_key)
      LEFT JOIN old_usage o USING (archive_key);

    UPDATE storage_usage SET bytes = bytes + delta WHERE singleton;
    RETURN NULL;
END
$$;

CREATE TRIGGER document_labels_storage_usage_insert
    AFTER INSERT ON document_labels
    REFERENCING NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION adjust_label_archive_storage_insert();

CREATE TRIGGER document_labels_storage_usage_update
    AFTER UPDATE ON document_labels
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
    FOR EACH STATEMENT EXECUTE FUNCTION adjust_label_archive_storage_update();

CREATE TRIGGER document_labels_storage_usage_delete
    AFTER DELETE ON document_labels
    REFERENCING OLD TABLE AS old_rows
    FOR EACH STATEMENT EXECUTE FUNCTION adjust_label_archive_storage_delete();

-- Repair counters created by the row trigger.  Assets are unique objects in
-- this schema; archives are shared by key and charged using one row per key.
-- The source tables have remained locked throughout the migration.
SELECT singleton FROM storage_usage WHERE singleton FOR UPDATE;

WITH archive_usage AS (
         SELECT DISTINCT ON (archive_key) archive_key, archive_bytes
         FROM document_labels
         WHERE archive_key IS NOT NULL
         ORDER BY archive_key, id
     ),
     total AS (
         SELECT (
             (SELECT COALESCE(sum(byte_length), 0)::bigint FROM document_assets)
             +
             (SELECT COALESCE(sum(archive_bytes), 0)::bigint FROM archive_usage)
         )::bigint AS bytes
     )
UPDATE storage_usage
SET bytes = total.bytes
FROM total
WHERE singleton;
