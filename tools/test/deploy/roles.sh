#!/usr/bin/env bash
set -Eeuo pipefail

# Exercise the shipped PostgreSQL first-boot role setup and host SCRAM policy
# inside a disposable PostgreSQL container. No deployment volume is reused.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
postgres_image="${DEPLOY_ROLES_POSTGRES_IMAGE:-postgres:17.11-alpine}"
command -v docker >/dev/null || { echo 'docker is required' >&2; exit 1; }
command -v openssl >/dev/null || { echo 'openssl is required' >&2; exit 1; }

work=$(mktemp -d /tmp/librepaper-deploy-roles.XXXXXX)
case "$work" in /tmp/librepaper-deploy-roles.*) ;; *) echo 'unsafe temporary path' >&2; exit 2;; esac
container="lp-deploy-roles-${work##*.}"
container_owned=0
cleanup() {
	local status=$?
	trap - EXIT
	if (( container_owned )); then
		if (( status != 0 )); then
			docker logs "$container" >"$work/postgres.log" 2>&1 || true
			echo "role integration diagnostics preserved at $work" >&2
		fi
		docker rm -f -v "$container" >/dev/null 2>&1 || true
	fi
	if [[ "${DEPLOY_ROLES_KEEP:-0}" == 1 || $status != 0 ]]; then
		echo "disposable PostgreSQL role fixture preserved at $work" >&2
	else
		rm -rf -- "$work"
	fi
	exit "$status"
}
trap cleanup EXIT

secret_dir="$work/secrets"
mkdir -m 700 "$secret_dir"
for secret in postgres_bootstrap_password database_owner_password database_app_password \
	database_backup_password database_metrics_password; do
	openssl rand -hex 32 >"$secret_dir/$secret"
	chmod 0444 "$secret_dir/$secret"
done

# Mount the exact init files installed by deploy/compose.yaml and provide the
# same secret paths, role names, SCRAM initdb options, and password encryption.
docker run -d --name "$container" \
	-e POSTGRES_DB=librepaper \
	-e POSTGRES_USER=librepaper_bootstrap \
	-e POSTGRES_PASSWORD_FILE=/run/secrets/postgres_bootstrap_password \
	-e 'POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256 --auth-local=trust' \
	-v "$secret_dir/postgres_bootstrap_password:/run/secrets/postgres_bootstrap_password:ro" \
	-v "$secret_dir/database_owner_password:/run/secrets/database_owner_password:ro" \
	-v "$secret_dir/database_app_password:/run/secrets/database_app_password:ro" \
	-v "$secret_dir/database_backup_password:/run/secrets/database_backup_password:ro" \
	-v "$secret_dir/database_metrics_password:/run/secrets/database_metrics_password:ro" \
	-v "$root/deploy/postgres/init-roles.sh:/docker-entrypoint-initdb.d/10-librepaper-roles.sh:ro" \
	-v "$root/deploy/postgres/roles.sql:/usr/local/share/librepaper/roles.sql:ro" \
	"$postgres_image" postgres -c password_encryption=scram-sha-256 >/dev/null
container_owned=1

ready=0
for _ in {1..90}; do
	if docker exec "$container" pg_isready -q -h 127.0.0.1 -U librepaper_bootstrap -d librepaper; then
		ready=1
		break
	fi
	sleep 1
done
[[ "$ready" == 1 ]] || { echo 'disposable PostgreSQL did not become ready' >&2; exit 1; }

# All client requests explicitly use TCP localhost, so they traverse the
# generated pg_hba.conf host rule instead of the initdb Unix-socket trust rule.
psql_as() {
	local role="$1" secret="$2"
	shift 2
	docker exec "$container" sh -c '
		PGPASSWORD=$(cat "$1")
		export PGPASSWORD
		shift
		exec "$@"
	' sh "/run/secrets/$secret" \
		psql -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -U "$role" -d librepaper "$@"
}

expect_sqlstate_denied() {
	local label="$1" role="$2" secret="$3" sql="$4" output="$work/$1.log"
	if psql_as "$role" "$secret" -v VERBOSITY=verbose -c "$sql" >"$output" 2>&1; then
		echo "$label unexpectedly succeeded" >&2
		cat "$output" >&2
		exit 1
	fi
	if ! grep -q '42501' "$output"; then
		echo "$label failed for a reason other than insufficient privilege" >&2
		cat "$output" >&2
		exit 1
	fi
}

expect_auth_denied() {
	local label="$1" role="$2" secret="$3" output="$work/$1.log"
	if psql_as "$role" "$secret" -c 'SELECT 1' >"$output" 2>&1; then
		echo "$label unexpectedly authenticated" >&2
		cat "$output" >&2
		exit 1
	fi
	if ! grep -Eqi 'password authentication failed|28P01' "$output"; then
		echo "$label failed without a password authentication error" >&2
		cat "$output" >&2
		exit 1
	fi
}

expect_auth_denied wrong_app_password librepaper_app postgres_bootstrap_password
expect_auth_denied metrics_credentials_as_app librepaper_app database_metrics_password

# Confirm first-boot setup installed SCRAM verifiers and a TCP HBA rule. The
# successful and failed role logins below independently exercise the rule.
scram_check=$(psql_as librepaper_bootstrap postgres_bootstrap_password -Atc \
	"SELECT bool_and(rolpassword LIKE 'SCRAM-SHA-256$%') FROM pg_authid WHERE rolname IN ('librepaper_owner','librepaper_app','librepaper_backup','librepaper_metrics')")
[[ "$scram_check" == t ]] || { echo 'expected all deployment roles to have SCRAM password verifiers' >&2; exit 1; }
hba_check=$(psql_as librepaper_bootstrap postgres_bootstrap_password -Atc \
	"SELECT EXISTS (SELECT 1 FROM pg_hba_file_rules WHERE type = 'host' AND auth_method = 'scram-sha-256' AND ('all' = ANY(database) OR 'librepaper' = ANY(database)))")
[[ "$hba_check" == t ]] || { echo 'expected a TCP SCRAM rule for librepaper' >&2; exit 1; }

psql_as librepaper_owner database_owner_password -c \
	"CREATE TABLE public.deploy_roles_probe (id BIGSERIAL PRIMARY KEY, value TEXT NOT NULL); INSERT INTO public.deploy_roles_probe(value) VALUES ('owner-row');" >/dev/null

# Default privileges created by roles.sql must give the app normal row and
# sequence DML while withholding every schema-changing path.
inserted_id=$(psql_as librepaper_app database_app_password -Atc \
	"INSERT INTO public.deploy_roles_probe(value) VALUES ('app-row') RETURNING id")
[[ "$inserted_id" =~ ^[0-9]+$ ]] || { echo 'app could not insert using the owner-created sequence' >&2; exit 1; }
app_rows=$(psql_as librepaper_app database_app_password -Atc \
	"SELECT count(*) FROM public.deploy_roles_probe WHERE value IN ('owner-row','app-row')")
[[ "$app_rows" == 2 ]] || { echo 'app SELECT did not see owner and app rows' >&2; exit 1; }
psql_as librepaper_app database_app_password -c \
	"UPDATE public.deploy_roles_probe SET value = 'app-updated' WHERE id = $inserted_id" >/dev/null
updated=$(psql_as librepaper_app database_app_password -Atc \
	"SELECT value FROM public.deploy_roles_probe WHERE id = $inserted_id")
[[ "$updated" == app-updated ]] || { echo 'app UPDATE did not persist' >&2; exit 1; }
psql_as librepaper_app database_app_password -c \
	"DELETE FROM public.deploy_roles_probe WHERE id = $inserted_id" >/dev/null
remaining=$(psql_as librepaper_app database_app_password -Atc \
	"SELECT count(*) FROM public.deploy_roles_probe WHERE id = $inserted_id")
[[ "$remaining" == 0 ]] || { echo 'app DELETE did not persist' >&2; exit 1; }

expect_sqlstate_denied app_create_table librepaper_app database_app_password \
	'CREATE TABLE public.deploy_roles_app_denied (id integer)'
expect_sqlstate_denied app_alter_table librepaper_app database_app_password \
	'ALTER TABLE public.deploy_roles_probe ADD COLUMN denied integer'
expect_sqlstate_denied app_set_role_owner librepaper_app database_app_password \
	'SET ROLE librepaper_owner'

# The backup role can read the owner-created probe and produce an actual dump,
# but has no write privilege even when given an existing table.
backup_rows=$(psql_as librepaper_backup database_backup_password -Atc \
	"SELECT count(*) FROM public.deploy_roles_probe WHERE value = 'owner-row'")
[[ "$backup_rows" == 1 ]] || { echo 'backup role could not read owner-created table data' >&2; exit 1; }
expect_sqlstate_denied backup_insert librepaper_backup database_backup_password \
	"INSERT INTO public.deploy_roles_probe(value) VALUES ('forbidden')"
docker exec "$container" sh -c '
	PGPASSWORD=$(cat /run/secrets/database_backup_password)
	export PGPASSWORD
	exec pg_dump -h 127.0.0.1 -U librepaper_backup -d librepaper --schema-only --no-owner -f /tmp/deploy-roles-schema.sql
'
docker exec "$container" test -s /tmp/deploy-roles-schema.sql

# pg_monitor grants statistics visibility but no membership or app data access.
metrics_stats=$(psql_as librepaper_metrics database_metrics_password -Atc \
	'SELECT count(*) >= 1 FROM pg_stat_activity WHERE datname = current_database()')
[[ "$metrics_stats" == t ]] || { echo 'metrics role could not read current database statistics' >&2; exit 1; }
expect_sqlstate_denied metrics_set_role_app librepaper_metrics database_metrics_password \
	'SET ROLE librepaper_app'
expect_sqlstate_denied metrics_read_app_table librepaper_metrics database_metrics_password \
	'SELECT count(*) FROM public.deploy_roles_probe'

echo 'deployment PostgreSQL SCRAM and role privilege acceptance passed'
