#!/usr/bin/env bash
set -Eeuo pipefail

# Create/drop run-scoped logins for the disposable recovery drill. A backup
# login is read-only; a restored app login gets only runtime DML privileges.
mode="${1:-}"
owner_url="${2:-}"
database="${3:-}"
url_file="${4:-}"
: "${owner_url:?usage: database-role.sh backup|app|drop OWNER_URL DATABASE URL_FILE}"
: "${database:?usage: database-role.sh backup|app|drop OWNER_URL DATABASE URL_FILE}"
: "${url_file:?usage: database-role.sh backup|app|drop OWNER_URL DATABASE URL_FILE}"
command -v psql >/dev/null
command -v node >/dev/null

current_database=$(psql "$owner_url" -XAt -v ON_ERROR_STOP=1 -c 'SELECT current_database()')
[[ "$current_database" == "$database" ]] || {
	echo "refusing backup role operation: expected $database, got $current_database" >&2
	exit 2
}

case "$mode" in
backup|app)
	[[ ! -e "$url_file" && ! -e "$url_file.role" ]] || {
		echo "refusing to overwrite backup role files: $url_file" >&2
		exit 2
	}
	suffix=$(node -e 'process.stdout.write(require("node:crypto").randomUUID().replaceAll("-", ""))')
	role="librepaper_${mode}_test_${suffix}"
	password=$(node -e 'process.stdout.write(require("node:crypto").randomBytes(32).toString("hex"))')
	psql "$owner_url" -X -v ON_ERROR_STOP=1 -v backup_role="$role" -v backup_password="$password" <<'SQL' >/dev/null
CREATE ROLE :"backup_role" LOGIN PASSWORD :'backup_password';
SQL
	psql "$owner_url" -X -v ON_ERROR_STOP=1 -v backup_role="$role" -v database="$database" <<'SQL' >/dev/null
GRANT CONNECT ON DATABASE :"database" TO :"backup_role";
GRANT USAGE ON SCHEMA public TO :"backup_role";
SQL
	if [[ "$mode" == backup ]]; then
		psql "$owner_url" -X -v ON_ERROR_STOP=1 -v backup_role="$role" <<'SQL' >/dev/null
GRANT SELECT ON ALL TABLES IN SCHEMA public TO :"backup_role";
GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO :"backup_role";
SQL
	else
		psql "$owner_url" -X -v ON_ERROR_STOP=1 -v backup_role="$role" <<'SQL' >/dev/null
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO :"backup_role";
REVOKE INSERT, UPDATE, DELETE ON _sqlx_migrations FROM :"backup_role";
REVOKE INSERT, DELETE ON deployment_writer FROM :"backup_role";
GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO :"backup_role";
SQL
	fi
	backup_url=$(node --input-type=module - "$owner_url" "$role" "$password" <<'JS'
const [, , value, username, secret] = process.argv;
const url = new URL(value);
url.username = username;
url.password = secret;
process.stdout.write(url.href);
JS
)
	printf '%s\n' "$backup_url" >"$url_file"
	printf '%s\n' "$role" >"$url_file.role"
	chmod 0400 "$url_file" "$url_file.role"
	;;
drop)
	if [[ -f "$url_file.role" ]]; then
		role=$(<"$url_file.role")
		psql "$owner_url" -X -v ON_ERROR_STOP=1 -v backup_role="$role" <<'SQL' >/dev/null
DROP OWNED BY :"backup_role";
DROP ROLE :"backup_role";
SQL
		rm -f -- "$url_file" "$url_file.role"
	fi
	;;
*)
	echo "usage: database-role.sh backup|app|drop OWNER_URL DATABASE URL_FILE" >&2
	exit 2
	;;
esac
