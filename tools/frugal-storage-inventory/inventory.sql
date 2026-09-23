-- Run with psql -X -v ON_ERROR_STOP=1 -f inventory.sql "$DATABASE_URL".
-- Every query is read-only. This intentionally targets the current schema
-- (migration 0005); it fails visibly on legacy schemas instead of blending
-- incompatible measurements.
\pset format unaligned
\pset fieldsep '\t'
\pset tuples_only on
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;

SELECT 'database' AS section, current_database(), version(),
       pg_database_size(current_database())::bigint AS database_bytes;

SELECT 'documents' AS section,
       count(*)::bigint AS total,
       count(*) FILTER (WHERE status='active')::bigint AS active,
       count(*) FILTER (WHERE status='deleting')::bigint AS deleting
FROM documents;

SELECT 'updates_source_history' AS section,
       count(*)::bigint AS rows,
       COALESCE(sum(octet_length(update_bytes)),0)::bigint AS payload_bytes,
       COALESCE(sum(pg_column_size(t)),0)::bigint AS row_bytes_including_tuple_overhead
FROM document_updates t;

SELECT 'assets' AS section,
       count(*)::bigint AS ref_count,
       count(DISTINCT storage_key)::bigint AS unique_keys,
       COALESCE(sum(byte_length),0)::bigint AS reference_bytes,
       COALESCE((SELECT sum(byte_length) FROM
                 (SELECT DISTINCT storage_key,byte_length FROM document_assets) u),0)::bigint AS unique_object_bytes,
       0::bigint AS unknown_size_rows
FROM document_assets;

SELECT 'snapshots_live_retired' AS section,
       count(*)::bigint AS rows,
       count(*) FILTER (WHERE delete_after IS NULL)::bigint AS live_rows,
       COALESCE(sum(snapshot_bytes) FILTER (WHERE delete_after IS NULL),0)::bigint AS live_known_bytes,
       count(*) FILTER (WHERE delete_after IS NOT NULL)::bigint AS retired_rows,
       COALESCE(sum(snapshot_bytes) FILTER (WHERE delete_after IS NOT NULL),0)::bigint AS retired_known_bytes,
       count(*) FILTER (WHERE snapshot_bytes IS NULL)::bigint AS unknown_size_rows,
       COALESCE(sum(pg_column_size(s)),0)::bigint AS catalogue_row_bytes
FROM document_snapshots s;

SELECT 'labels_archives' AS section,
       count(*) FILTER (WHERE archive_key IS NOT NULL)::bigint AS ref_count,
       count(DISTINCT archive_key)::bigint AS unique_keys,
       COALESCE(sum(archive_bytes),0)::bigint AS reference_bytes,
       COALESCE((SELECT sum(archive_bytes) FROM
                 (SELECT DISTINCT ON (archive_key) archive_key,archive_bytes
                  FROM document_labels WHERE archive_key IS NOT NULL
                  ORDER BY archive_key,id) u),0)::bigint AS unique_object_bytes,
       count(*) FILTER (WHERE archive_key IS NOT NULL AND archive_bytes IS NULL)::bigint AS unknown_size_rows
FROM document_labels;

SELECT 'generated_artifact_payloads' AS section,
       (SELECT count(*)::bigint FROM document_proposals) AS proposal_rows,
       (SELECT COALESCE(sum(octet_length(branch_bytes)),0)::bigint FROM document_proposals) AS proposal_branch_bytes,
       (SELECT count(*)::bigint FROM document_proposal_hunks) AS decided_hunks,
       (SELECT COALESCE(sum(octet_length(update_bytes)),0)::bigint FROM document_updates) AS history_payload_bytes;

SELECT 'accounted_object_bytes' AS section, bytes::bigint FROM storage_usage WHERE singleton;

-- pg_total_relation_size includes heap, TOAST and indexes. The index line
-- isolates index bytes; the database line above also includes system catalogs
-- and free space. These are physical PostgreSQL files, unlike object metadata.
SELECT 'postgres_relation_sizes' AS section,
       schemaname, relname,
       pg_relation_size(format('%I.%I',schemaname,relname))::bigint AS heap_bytes,
       pg_indexes_size(format('%I.%I',schemaname,relname))::bigint AS index_bytes,
       pg_total_relation_size(format('%I.%I',schemaname,relname))::bigint AS total_bytes
FROM pg_stat_user_tables
ORDER BY pg_total_relation_size(format('%I.%I',schemaname,relname)) DESC, relname;

-- Object-store physical totals are not queryable from the catalogue. For the
-- filesystem backend, separately run `du -sb -- "$LIBREPAPER_DATA/objects"`
-- and `find "$LIBREPAPER_DATA/objects" -type f -printf '%s\n' | awk ...`.
-- For S3, collect bucket inventory/listing totals under the operator's IAM
-- permissions. Compare those totals with the named object rows above; orphan
-- blobs and provider/versioning overhead are not represented in this query.
-- WAL physical bytes are also excluded. If the server grants pg_ls_waldir(),
-- run: SELECT COALESCE(sum(size),0)::bigint FROM pg_ls_waldir(); otherwise
-- report WAL bytes as unknown. pg_stat_wal counters are cumulative writes,
-- not current disk usage.

COMMIT;
