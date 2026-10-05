#!/usr/bin/env bash
set -Eeuo pipefail

# Exercise the shipped backup image against disposable application data and
# local-only backend services. Every Docker object has this run's unique name.
: "${LIBREPAPER_BIN:?set to a built librepaper executable}"
: "${LIBREPAPER_SOURCE_URL:?set a disposable PostgreSQL URL}"
: "${LIBREPAPER_BACKUP_IMAGE:?set the locally built backup image}"
command -v docker >/dev/null
command -v node >/dev/null
command -v ssh-keygen >/dev/null

work=$(mktemp -d /tmp/librepaper-backup-sidecar.XXXXXX)
prefix="lp-backup-$(basename "$work" | tr -cd 'a-zA-Z0-9' | cut -c1-20)"
sidecar="${prefix}-sidecar"
cleanup() {
	local status=$?
	trap - EXIT
	docker rm -f "$sidecar" >/dev/null 2>&1 || true
	for name in "${backend_containers[@]:-}"; do docker rm -f "$name" >/dev/null 2>&1 || true; done
	if [[ "${BACKUP_SIDECAR_KEEP:-0}" == 1 || $status != 0 ]]; then
		echo "synthetic sidecar fixture preserved at $work" >&2
	else
		rm -rf -- "$work"
	fi
	exit "$status"
}
backend_containers=()
trap cleanup EXIT
mkdir -m 700 "$work/source-data" "$work/repos"

source_db=$(psql "$LIBREPAPER_SOURCE_URL" -XAt -v ON_ERROR_STOP=1 -c 'select current_database()')
[[ "$source_db" == backup_fixture ]] || { echo "refusing source database: expected backup_fixture, got $source_db" >&2; exit 2; }
source_tables=$(psql "$LIBREPAPER_SOURCE_URL" -XAt -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM pg_tables WHERE schemaname='public'")
[[ "$source_tables" == 0 ]] || { echo "refusing nonempty source database ($source_tables public tables)" >&2; exit 2; }

# The existing drill fixture creates five projects with snapshots and revisions.
LIBREPAPER_SOURCE_URL="$LIBREPAPER_SOURCE_URL" node tools/test/backup/fixture.mjs \
	"$LIBREPAPER_BIN" "$LIBREPAPER_SOURCE_URL" "$work/source-data"
chmod -R a+rX "$work/source-data"
chmod 0777 "$work/repos"

cat >"$work/disabled.toml" <<'TOML'
[storage]
directory = "/var/lib/librepaper"
database_url = { env = "LIBREPAPER_DATABASE_URL" }
fsync = false
TOML
cat >"$work/enabled.toml" <<'TOML'
[storage]
directory = "/var/lib/librepaper"
database_url = { env = "LIBREPAPER_DATABASE_URL" }
fsync = false

[resticprofile]
repository = "/var/backups/librepaper/repository"
password-file = "/test/repository-password"

[resticprofile.retention]
keep-last = 1
prune = true
max-unused = "0"
TOML
printf '%s\n' 'synthetic-only-password' >"$work/repository-password"
chmod 0644 "$work/repository-password"

db_url="$LIBREPAPER_SOURCE_URL"
common=()
if [[ "$db_url" == *"127.0.0.1"* ]]; then
	common+=(--network host)
fi

docker run -d --name "$sidecar" "${common[@]}" \
	--tmpfs /run/librepaper-backup:uid=10001,gid=65534,mode=0700 \
	-v "$work:/test:rw" -v "$work/source-data:/var/lib/librepaper:rw" \
	-v "$work/repos:/var/backups/librepaper:rw" \
	-v "$work/disabled.toml:/etc/librepaper/config.toml:ro" \
	-e LIBREPAPER_DATABASE_URL="$db_url" "$LIBREPAPER_BACKUP_IMAGE" >/dev/null
sleep 2
docker exec "$sidecar" sh -c 'test "$(cat /var/backups/librepaper/metrics/librepaper_backup.prom | sed -n "s/^librepaper_backup_enabled //p")" = 0'
docker rm -f "$sidecar" >/dev/null

launch_enabled() {
	docker rm -f "$sidecar" >/dev/null 2>&1 || true
	docker run -d --name "$sidecar" "${common[@]}" \
		--tmpfs /run/librepaper-backup:uid=10001,gid=65534,mode=0700 \
		-v "$work:/test:rw" -v "$work/source-data:/var/lib/librepaper:rw" \
		-v "$work/repos:/var/backups/librepaper:rw" \
		-v "$work/enabled.toml:/etc/librepaper/config.toml:ro" \
		-e LIBREPAPER_DATABASE_URL="$db_url" "$LIBREPAPER_BACKUP_IMAGE" >/dev/null
	for _ in {1..30}; do
		if docker exec "$sidecar" test -s /run/librepaper-backup/crontab 2>/dev/null; then return 0; fi
		sleep 1
	done
	docker logs "$sidecar" >&2
	return 1
}

launch_enabled
docker exec "$sidecar" resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup
docker exec "$sidecar" restic -r /var/backups/librepaper/repository --password-file /test/repository-password snapshots --json >"$work/snapshots.json"
node --input-type=module - "$work/snapshots.json" <<'JS'
import assert from 'node:assert/strict';
import fs from 'node:fs';
const snapshots = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
assert.equal(snapshots.length, 1, 'successful export creates one repository snapshot');
assert.ok(snapshots[0].paths.includes('/var/backups/librepaper/current'));
console.log('backup sidecar exported the verified application bundle and created a snapshot');
JS

# A failed database connection must fail the export hook and leave snapshot
# count and last-success metrics unchanged.
if docker exec -e LIBREPAPER_DATABASE_URL=postgresql://invalid@127.0.0.1:1/missing \
	"$sidecar" resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup; then
	echo 'expected backup to fail when its database is unreachable' >&2
	exit 1
fi
docker exec "$sidecar" restic -r /var/backups/librepaper/repository --password-file /test/repository-password snapshots --json >"$work/failed-snapshots.json"
node --input-type=module - "$work/snapshots.json" "$work/failed-snapshots.json" <<'JS'
import assert from 'node:assert/strict';
import fs from 'node:fs';
const before = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const after = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
assert.equal(after.length, before.length, 'failed export must not create a snapshot');
JS
docker exec "$sidecar" sh -c 'grep -q "librepaper_backup_job_last_result_success{task=\"backup\"} 0" /var/backups/librepaper/metrics/librepaper_backup.prom'

# A later good run must recover and apply after-backup forget/prune. The
# fixture's keep-last=1 policy makes this assertion deterministic.
docker exec "$sidecar" resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup
docker exec "$sidecar" restic -r /var/backups/librepaper/repository --password-file /test/repository-password snapshots --json >"$work/retained-snapshots.json"
node --input-type=module - "$work/retained-snapshots.json" <<'JS'
import assert from 'node:assert/strict';
import fs from 'node:fs';
const snapshots = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
assert.equal(snapshots.length, 1, 'after-backup retention keeps only the configured latest snapshot');
JS

# A real repository check also exercises the profile hooks and shared lock.
docker exec "$sidecar" resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile check
docker exec "$sidecar" sh -c 'grep -q "librepaper_backup_job_last_result_success{task=\"check\"} 1" /var/backups/librepaper/metrics/librepaper_backup.prom'

# Restore the actual sidecar snapshot into a fresh database and directory.
docker exec "${POSTGRES_CONTAINER:?}" createdb -U postgres backup_restored
docker exec "$sidecar" restic -r /var/backups/librepaper/repository \
	--password-file /test/repository-password restore latest --target /test/restic-restore
cat >"$work/restore.toml" <<TOML
[storage]
directory = "$work/restore-data"
database_url = { env = "LIBREPAPER_RESTORE_URL" }
fsync = false
TOML
LIBREPAPER_RESTORE_URL="postgresql://postgres:drill@127.0.0.1:${POSTGRES_PORT}/backup_restored" \
	"$LIBREPAPER_BIN" admin restore --config "$work/restore.toml" \
	"$work/restic-restore/var/backups/librepaper/current" "$work/restore-data"

echo 'local backend acceptance passed'

# Exercise real remote backends with synthetic-only credentials. Service ports
# bind to loopback; no host service, real account, or production repository is
# contacted. Each profile is mounted as a separate file so Compose-style
# read-only bind replacement behavior is avoided.
run_remote_profile() {
	local config="$1"
	docker rm -f "$sidecar" >/dev/null 2>&1 || true
	docker run -d --name "$sidecar" "${common[@]}" \
		--tmpfs /run/librepaper-backup:uid=10001,gid=65534,mode=0700 \
		-v "$work:/test:rw" -v "$work/source-data:/var/lib/librepaper:rw" \
		-v "$work/repos:/var/backups/librepaper:rw" \
		-v "$config:/etc/librepaper/config.toml:ro" \
		-e LIBREPAPER_DATABASE_URL="$db_url" "$LIBREPAPER_BACKUP_IMAGE" >/dev/null
	for _ in {1..30}; do
		if docker exec "$sidecar" test -s /run/librepaper-backup/crontab 2>/dev/null; then break; fi
		sleep 1
	done
	docker exec "$sidecar" resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile backup
	docker exec "$sidecar" resticprofile -c /etc/resticprofile/profiles.toml -n resticprofile check
}

if [[ "${BACKUP_TEST_BACKENDS:-1}" != 0 ]]; then
	# SFTP server with a newly generated key that is confined to this fixture.
	ssh-keygen -q -t ed25519 -N '' -f "$work/id_ed25519"
	chmod 0644 "$work/id_ed25519.pub"
	name="${prefix}-sftp"
	backend_containers+=("$name")
	docker run -d --name "$name" -p 127.0.0.1::22 \
		-v "$work/id_ed25519.pub:/home/fixture/.ssh/keys/id_ed25519.pub:ro" \
		"${BACKUP_TEST_SFTP_IMAGE:-atmoz/sftp:alpine}" \
		fixture:fixture-password:1001:1001:upload >/dev/null
	sftp_port=$(docker port "$name" 22/tcp | head -1 | sed 's/.*://')
	cat >"$work/sftp.toml" <<TOML
[storage]
directory = "/var/lib/librepaper"
database_url = { env = "LIBREPAPER_DATABASE_URL" }
fsync = false

[resticprofile]
repository = "sftp://fixture@127.0.0.1:$sftp_port/upload/repository"
password-file = "/test/repository-password"
option = ["sftp.args=-i /test/id_ed25519 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null"]
TOML
	run_remote_profile "$work/sftp.toml"
	echo 'SFTP backend acceptance passed'

	# MinIO receives disposable credentials and its own repository namespace.
	name="${prefix}-minio"
	backend_containers+=("$name")
	docker run -d --name "$name" -p 127.0.0.1::9000 \
		-e MINIO_ROOT_USER=fixture-access -e MINIO_ROOT_PASSWORD=fixture-secret-password \
		"${BACKUP_TEST_MINIO_IMAGE:-minio/minio:latest}" server /data --address :9000 >/dev/null
	minio_port=$(docker port "$name" 9000/tcp | head -1 | sed 's/.*://')
	cat >"$work/minio.toml" <<TOML
[storage]
directory = "/var/lib/librepaper"
database_url = { env = "LIBREPAPER_DATABASE_URL" }
fsync = false

[resticprofile]
repository = "s3:http://127.0.0.1:$minio_port/librepaper-fixture"
password-file = "/test/repository-password"
option = ["s3.bucket-lookup=path"]

[resticprofile.env]
AWS_ACCESS_KEY_ID = "fixture-access"
AWS_SECRET_ACCESS_KEY = "fixture-secret-password"
AWS_DEFAULT_REGION = "us-east-1"
TOML
	run_remote_profile "$work/minio.toml"
	echo 'S3-compatible MinIO backend acceptance passed'

	# REST server uses a synthetic Basic Auth account and private-repo namespace.
	name="${prefix}-rest"
	backend_containers+=("$name")
	docker run -d --name "$name" -p 127.0.0.1::8000 \
		-e OPTIONS=--private-repos "${BACKUP_TEST_REST_IMAGE:-restic/rest-server:0.14.0}" >/dev/null
	docker exec "$name" create_user fixture fixture-rest-password >/dev/null
	rest_port=$(docker port "$name" 8000/tcp | head -1 | sed 's/.*://')
	cat >"$work/rest.toml" <<TOML
[storage]
directory = "/var/lib/librepaper"
database_url = { env = "LIBREPAPER_DATABASE_URL" }
fsync = false

[resticprofile]
repository = "rest:http://fixture:fixture-rest-password@127.0.0.1:$rest_port/fixture/repository"
password-file = "/test/repository-password"
TOML
	run_remote_profile "$work/rest.toml"
	echo 'REST backend acceptance passed'
fi
