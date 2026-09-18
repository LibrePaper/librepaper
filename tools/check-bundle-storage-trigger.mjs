// Run against a local PostgreSQL container. All fixtures and migrations live in
// a temporary schema in one rolled-back transaction; no application rows change.
import { readFileSync, readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
const migrations = new URL('../crates/librepaper/migrations/postgres/', import.meta.url);
const schema = `bundle_trigger_test_${randomBytes(8).toString('hex')}`;
const before = readdirSync(migrations).filter(name => name.endsWith('.sql') && Number(name.slice(0, 4)) <= 17).sort()
  .map(name => readFileSync(new URL(name, migrations), 'utf8')).join('\n');
const repair = readFileSync(new URL('0018_bundle_storage_trigger.sql', migrations), 'utf8');
const insert = `INSERT INTO bundle_files VALUES ('00000000-0000-0000-0000-000000000003', 'index.html', 'rendered', decode(repeat('00',32),'hex'), 10, 'text/html');`;
const sql = `BEGIN;
CREATE SCHEMA ${schema};
SET LOCAL search_path TO ${schema};
${before}
INSERT INTO accounts(id,kind,handle,display_name,status)
VALUES ('00000000-0000-0000-0000-000000000001','system','test','Test','active');
INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path)
VALUES ('00000000-0000-0000-0000-000000000002','test','00000000-0000-0000-0000-000000000001','owned','Test','active','latex','main.tex');
INSERT INTO bundles(id,document_id,request_key,request_digest,manifest_key,manifest_digest,rendered_by_label)
VALUES ('00000000-0000-0000-0000-000000000003','00000000-0000-0000-0000-000000000002','test',decode(repeat('00',32),'hex'),'manifest',decode(repeat('00',32),'hex'),'Test');
DO $$ BEGIN
  ${insert}
  RAISE EXCEPTION 'Expected migration 17 to reproduce the missing-table error';
EXCEPTION WHEN undefined_table THEN
  IF SQLERRM NOT LIKE '%publication_files%' THEN RAISE; END IF;
END $$;
${repair}
${insert}
DO $$ BEGIN ASSERT (SELECT bytes FROM storage_usage) = 10, 'first object counted'; END $$;
INSERT INTO bundle_files SELECT bundle_id,'copy.html',storage_key,digest,byte_length,media_type FROM bundle_files;
DO $$ BEGIN ASSERT (SELECT bytes FROM storage_usage) = 10, 'shared object counted once'; END $$;
DELETE FROM bundle_files WHERE path='index.html';
DO $$ BEGIN ASSERT (SELECT bytes FROM storage_usage) = 10, 'shared object retained'; END $$;
DELETE FROM bundle_files WHERE path='copy.html';
DO $$ BEGIN ASSERT (SELECT bytes FROM storage_usage) = 0, 'last reference refunded'; END $$;
INSERT INTO document_assets(id,document_id,storage_key,digest,byte_length,media_type)
VALUES ('00000000-0000-0000-0000-000000000004','00000000-0000-0000-0000-000000000002','figure',decode(repeat('01',32),'hex'),20,'image/png');
INSERT INTO bundle_files SELECT '00000000-0000-0000-0000-000000000003','figure.png',storage_key,digest,byte_length,media_type FROM document_assets;
DO $$ BEGIN ASSERT (SELECT bytes FROM storage_usage) = 20, 'document asset not double counted'; END $$;
DELETE FROM bundle_files;
DO $$ BEGIN ASSERT (SELECT bytes FROM storage_usage) = 20, 'document asset retained after bundle deletion'; END $$;
ROLLBACK;`;
const result = spawnSync('docker', ['exec', '-i', process.env.LIBREPAPER_TEST_POSTGRES_CONTAINER || 'librepaper-postgres',
  'psql', '-U', 'postgres', '-d', 'postgres', '-v', 'ON_ERROR_STOP=1', '-q'], { input: sql, encoding: 'utf8' });
if (result.error || result.status !== 0) throw new Error(result.error?.message || result.stderr || result.stdout);
console.log('bundle storage trigger: reproduced migration 17 failure; migration 18 insert, delete and shared-object accounting passed');
