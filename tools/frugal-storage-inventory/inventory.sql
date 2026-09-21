BEGIN READ ONLY;

SELECT 'meta' AS section, current_database(), version(),
       pg_size_pretty(pg_database_size(current_database())),
       pg_database_size(current_database())::bigint;

SELECT 'documents' AS section,
       count(*)::bigint AS total,
       count(*) FILTER (WHERE status='active')::bigint AS active,
       count(*) FILTER (WHERE status='deleting')::bigint AS deleting,
       count(*) FILTER (WHERE slug LIKE 'starter-%')::bigint AS starters
FROM documents;

SELECT 'starter_opening' AS section,
       count(*)::bigint AS starter_docs,
       count(*) FILTER (WHERE m.opened_at IS NOT NULL)::bigint AS opened,
       count(*) FILTER (WHERE m.opened_at IS NULL)::bigint AS never_opened,
       count(DISTINCT d.owner_id)::bigint AS starter_owners
FROM documents d
LEFT JOIN document_marks m
  ON m.document_id=d.id AND m.account_id=d.owner_id
WHERE d.slug LIKE 'starter-%';

SELECT 'assets' AS section,
       count(*)::bigint AS rows,
       COALESCE(sum(byte_length),0)::bigint AS reference_bytes,
       count(DISTINCT (digest,byte_length))::bigint AS global_unique_pairs,
       COALESCE((SELECT sum(byte_length)
                 FROM (SELECT DISTINCT digest,byte_length FROM document_assets) u),0)::bigint
         AS global_unique_bytes,
       count(DISTINCT storage_key)::bigint AS keys
FROM document_assets;

SELECT 'starter_assets' AS section,
       count(a.*)::bigint AS rows,
       COALESCE(sum(a.byte_length),0)::bigint AS bytes,
       count(DISTINCT (a.digest,a.byte_length))::bigint AS unique_pairs,
       COALESCE((SELECT sum(byte_length)
                 FROM (SELECT DISTINCT a2.digest,a2.byte_length
                       FROM document_assets a2
                       JOIN documents d2 ON d2.id=a2.document_id
                       WHERE d2.slug LIKE 'starter-%') u),0)::bigint
         AS unique_starter_bytes
FROM document_assets a
JOIN documents d ON d.id=a.document_id
WHERE d.slug LIKE 'starter-%';

SELECT EXISTS (SELECT 1 FROM information_schema.tables
               WHERE table_schema='public' AND table_name='document_snapshots')
       AS has_snapshots \gset inventory_
SELECT EXISTS (SELECT 1 FROM information_schema.tables
               WHERE table_schema='public' AND table_name='superseded_bases')
       AS has_superseded \gset inventory_
\if :inventory_has_snapshots
SELECT 'snapshots' AS section,
       count(*)::bigint AS rows,
       COALESCE(sum(snapshot_bytes),0)::bigint AS known_bytes,
       count(*) FILTER (WHERE snapshot_bytes IS NULL)::bigint AS unknown_size_rows,
       count(*) FILTER (WHERE delete_after IS NULL)::bigint AS current,
       count(*) FILTER (WHERE delete_after IS NOT NULL)::bigint AS retired
FROM document_snapshots;
\else
SELECT 'snapshots_legacy' AS section,
       count(*)::bigint AS rows,
       COALESCE(sum(snapshot_bytes),0)::bigint AS bytes,
       count(*)::bigint AS current,
       0::bigint AS retired
FROM document_bases;
\if :inventory_has_superseded
SELECT 'snapshots_legacy_retired' AS section,
       count(*)::bigint AS rows,
       NULL::bigint AS bytes,
       0::bigint AS current,
       count(*)::bigint AS retired
FROM superseded_bases;
\else
SELECT 'snapshots_legacy_retired' AS section,
       0::bigint AS rows,
       NULL::bigint AS bytes,
       0::bigint AS current,
       0::bigint AS retired;
\endif
\endif

SELECT 'updates' AS section,
       count(*)::bigint AS rows,
       COALESCE(sum(octet_length(update_bytes)),0)::bigint AS bytes
FROM document_updates;

SELECT 'archives' AS section,
       count(*) FILTER (WHERE archive_key IS NOT NULL)::bigint AS refs,
       count(DISTINCT archive_key)::bigint AS unique_keys,
       COALESCE(sum(archive_bytes),0)::bigint AS ref_bytes,
       COALESCE((SELECT sum(archive_bytes)
                 FROM (SELECT DISTINCT ON (archive_key)
                              archive_key,archive_bytes
                       FROM document_labels
                       WHERE archive_key IS NOT NULL
                       ORDER BY archive_key,id) u),0)::bigint AS unique_bytes
FROM document_labels;

SELECT 'accounting' AS section, bytes
FROM storage_usage WHERE singleton;

SELECT 'relations' AS section,
       COALESCE(sum(pg_total_relation_size(quote_ident(schemaname)||'.'||quote_ident(relname))),0)::bigint AS bytes
FROM pg_stat_user_tables s;

COMMIT;
