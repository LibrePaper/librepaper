#!/usr/bin/env bash
set -Eeuo pipefail

# Intentionally bound to the dedicated local fixture databases made for this
# drill. It refuses any other database names before writing anything.
: "${LIBREPAPER_BIN:?set to the built librepaper executable}"
: "${LIBREPAPER_SOURCE_URL:?set the disposable frugal_fixture URL}"
: "${LIBREPAPER_RESTORE_URL:?set the disposable frugal_restored URL}"
: "${AGE_RECIPIENT:?set an age recipient; the matching identity remains operator-managed}"
: "${AGE_IDENTITY:?set the path to the operator-managed age identity}"
command -v age >/dev/null
command -v tar >/dev/null
command -v psql >/dev/null
command -v node >/dev/null
command -v pg_dump >/dev/null
command -v pg_restore >/dev/null
test -x "$LIBREPAPER_BIN"
test -r "$AGE_IDENTITY"

source_db=$(psql "$LIBREPAPER_SOURCE_URL" -XAt -v ON_ERROR_STOP=1 -c 'select current_database()')
restore_db=$(psql "$LIBREPAPER_RESTORE_URL" -XAt -v ON_ERROR_STOP=1 -c 'select current_database()')
[[ "$source_db" == frugal_fixture ]] || { echo "refusing source database: expected frugal_fixture, got $source_db" >&2; exit 2; }
[[ "$restore_db" == frugal_restored ]] || { echo "refusing restore database: expected frugal_restored, got $restore_db" >&2; exit 2; }
[[ "$source_db" != "$restore_db" ]] || { echo 'source and restore databases must differ' >&2; exit 2; }
source_tables=$(psql "$LIBREPAPER_SOURCE_URL" -XAt -v ON_ERROR_STOP=1 -c \
  "SELECT count(*) FROM pg_tables WHERE schemaname='public'")
[[ "$source_tables" == 0 ]] || { echo "refusing nonempty source database ($source_tables public tables)" >&2; exit 2; }

work=$(mktemp -d /tmp/librepaper-frugal-recovery.XXXXXX)
case "$work" in /tmp/librepaper-frugal-recovery.*) ;; *) echo 'unsafe temporary path' >&2; exit 2;; esac
cleanup() {
  status=$?
  trap - EXIT
  case "$work" in
    /tmp/librepaper-frugal-recovery.*)
      if [[ "${FRUGAL_KEEP_DRILL:-0}" == 1 ]]; then
        rm -rf -- "$work/backup" "$work/recovery.tar.gz" "$work/recovered.tar.gz" \
          "$work/source-signature" "$work/restored-signature"
        if (( status != 0 )); then
          echo "preserved failed synthetic fixture at $work for inspection" >&2
        fi
      else
        rm -rf -- "$work"
      fi
      ;;
  esac
  exit "$status"
}
trap cleanup EXIT
mkdir -m 700 "$work/source-data" "$work/restored-data"

# Seed a synthetic current-schema deployment with real source history and
# bundled assets. No production data or existing deployment directory is read.
"$LIBREPAPER_BIN" admin seed \
  --database-url "$LIBREPAPER_SOURCE_URL" \
  --data-directory "$work/source-data" \
  --simulate-activity 7

signature_sql="SELECT jsonb_build_object(
  'documents',(SELECT count(*) FROM documents),
  'updates',(SELECT count(*) FROM document_updates),
  'update_payload_bytes',(SELECT COALESCE(sum(octet_length(update_bytes)),0) FROM document_updates),
  'update_payload_md5',(SELECT md5(COALESCE(string_agg(encode(update_bytes,'hex'),'' ORDER BY document_id,update_sequence),'')) FROM document_updates),
  'assets',(SELECT count(*) FROM document_assets),
  'asset_refs',(SELECT md5(COALESCE(string_agg(storage_key||':'||encode(digest,'hex')||':'||byte_length::text,'|' ORDER BY storage_key),'')) FROM document_assets),
  'snapshots',(SELECT count(*) FROM document_snapshots),
  'labels',(SELECT count(*) FROM document_labels)
)::text"
psql "$LIBREPAPER_SOURCE_URL" -XAt -v ON_ERROR_STOP=1 -c "$signature_sql" >"$work/source-signature"

"$LIBREPAPER_BIN" admin backup create \
  --database-url "$LIBREPAPER_SOURCE_URL" \
  --data-directory "$work/source-data" \
  --id frugal-recovery-drill "$work/backup"

# The portable recovery point is encrypted with the operator's public age
# recipient. The private identity is used only to decrypt into this 0700 temp
# directory and is never copied into the bundle or restored data directory.
tar -C "$work" -czf "$work/recovery.tar.gz" backup
age -r "$AGE_RECIPIENT" -o "$work/recovery.tar.gz.age" "$work/recovery.tar.gz"
node --input-type=module - "$work" <<'JSSIZES'
import fs from 'node:fs';
import path from 'node:path';
const root=process.argv[2], size=p=>fs.statSync(path.join(root,p)).size;
console.log(JSON.stringify({event:'backup_sizes',database_dump_bytes:size('backup/database.dump'),manifest_bytes:size('backup/manifest.json'),compressed_plaintext_bytes:size('recovery.tar.gz'),encrypted_bytes:size('recovery.tar.gz.age')}));
JSSIZES
rm -- "$work/recovery.tar.gz"
rm -rf -- "$work/backup"
age -d -i "$AGE_IDENTITY" -o "$work/recovered.tar.gz" "$work/recovery.tar.gz.age"
tar -tzf "$work/recovered.tar.gz" | awk 'BEGIN{ok=1} $0 !~ /^backup(\/|$)/ {ok=0} END{exit !ok}'
tar -C "$work" -xzf "$work/recovered.tar.gz"
node --input-type=module - "$work/backup" <<'JSVERIFY'
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
const root=process.argv[2], m=JSON.parse(fs.readFileSync(path.join(root,'manifest.json')));
const digest=b=>createHash('sha256').update(b).digest('hex');
assert.equal(m.verified,true);
assert.equal(m.format,'librepaper-postgres-backup-v1');
assert.equal(digest(fs.readFileSync(path.join(root,m.database))),m.database_sha256);
assert.ok(m.references.length,'fixture contains no object references');
for(const ref of m.references){
  const body=fs.readFileSync(path.join(root,m.objects,ref.key));
  if(ref.bytes) assert.equal(body.length,ref.bytes,ref.key);
  if(ref.sha256) assert.equal(digest(body),ref.sha256,ref.key);
}
console.log('encrypted recovery point decrypted and verified: '+m.references.length+' referenced objects');
JSVERIFY

# Restore is allowed only into the named fresh database. The CLI itself also
# enforces that public has no tables and the destination directory is absent.
restore_tables=$(psql "$LIBREPAPER_RESTORE_URL" -XAt -v ON_ERROR_STOP=1 -c \
  "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind='r'")
[[ "$restore_tables" == 0 ]] || { echo "refusing nonempty restore database ($restore_tables public tables)" >&2; exit 2; }
rmdir "$work/restored-data"
"$LIBREPAPER_BIN" admin backup restore \
  --database-url "$LIBREPAPER_RESTORE_URL" \
  --data-directory "$work/restored-data" \
  "$work/backup" "$work/restored-data"

psql "$LIBREPAPER_RESTORE_URL" -XAt -v ON_ERROR_STOP=1 -c "$signature_sql" >"$work/restored-signature"
cmp "$work/source-signature" "$work/restored-signature"
node --input-type=module - "$work/backup" "$work/restored-data/objects" <<'JSVERIFY'
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
const [backup,restored]=process.argv.slice(2);
const m=JSON.parse(fs.readFileSync(path.join(backup,'manifest.json')));
for(const ref of m.references){
  assert.deepEqual(fs.readFileSync(path.join(backup,m.objects,ref.key)),fs.readFileSync(path.join(restored,ref.key)),ref.key);
}
console.log('restore verification passed: database history signatures and all object bytes match');
JSVERIFY
if [[ "${FRUGAL_KEEP_DRILL:-0}" == 1 ]]; then
  echo "preserved synthetic fixture at $work (source-data=$work/source-data, restored-data=$work/restored-data)"
fi
