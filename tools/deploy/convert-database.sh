#!/usr/bin/env bash
# TEMPORARY. One-time conversion of the production PostgreSQL cluster from the
# scoped-role kit to the socket-trust kit, in place, in one transaction, with
# no data movement (SPEC-simple-deploy-kit.md, "One-time conversion"). Delete
# this file, tools/test/deploy/convert.sh, the convert-database subcommand of
# tools/deploy/production and the deploy-convert suite entry once production
# is converted.
#
# Runs ON THE HOST, in the kit directory, with no arguments:
#   ssh "$HOST" 'cd librepaper && bash -s' < tools/deploy/convert-database.sh
#
# Already in place when it starts:
#   - the new kit files, .env with COMPOSE_PROFILES=monitoring,
#     compose.override.yaml and the secrets, in the current directory
#   - the old containers, still running under the same Compose project
#   - $LIBREPAPER_CONVERT_DIR/host-files-before.tar.gz, a tar of the whole
#     kit directory taken before the new kit was synced (the directory is the
#     archive's top-level entry)
#
# Environment:
#   LIBREPAPER_CONVERT_DIR              default $HOME/librepaper-convert; the
#                                       script writes librepaper.dump,
#                                       before.txt and after.txt there
#   LIBREPAPER_CONVERT_INJECT_FAILURE   rehearsal only. When exactly 1, a
#                                       failing statement is appended to the
#                                       first SQL block so the rehearsal can
#                                       prove that the transaction rolls
#                                       back. Nothing sets it by default and
#                                       ssh does not forward the environment,
#                                       so it is never set on production.
#
# If it stops before the first SQL block commits, nothing changed in the
# database: untar host-files-before.tar.gz over the kit directory and run
# docker compose up -d. Never docker compose down, down --volumes or prune on
# the production host: Caddy also serves another app from caddy/local.d.
set -Eeuo pipefail

say() { printf 'convert-database: %s\n' "$*"; }
die() { printf 'convert-database: %s\n' "$*" >&2; exit 1; }

# psql_as ROLE DATABASE [psql arguments]: psql inside the postgres container,
# over its Unix socket. The SQL arrives on standard input.
psql_as() {
	local role=$1 database=$2
	shift 2
	docker compose exec -T postgres psql -XAtq -v ON_ERROR_STOP=1 -U "$role" -d "$database" "$@"
}

sort_lines() { printf '%s\n' "$1" | sed '/^$/d' | LC_ALL=C sort; }

# owned_objects ROLE DATABASE: one line per relation, type, schema, function
# or database that ROLE owns. DROP OWNED would drop every one of them.
owned_objects() {
	psql_as librepaper_bootstrap "$2" -v "owner=$1" -f - <<'SQL'
SELECT 'relation ' || n.nspname || '.' || c.relname
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relowner = (SELECT oid FROM pg_roles WHERE rolname = :'owner')
UNION ALL
SELECT 'type ' || n.nspname || '.' || t.typname
FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
WHERE t.typowner = (SELECT oid FROM pg_roles WHERE rolname = :'owner')
UNION ALL
SELECT 'schema ' || nspname
FROM pg_namespace
WHERE nspowner = (SELECT oid FROM pg_roles WHERE rolname = :'owner')
UNION ALL
SELECT 'function ' || n.nspname || '.' || p.proname
FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
WHERE p.proowner = (SELECT oid FROM pg_roles WHERE rolname = :'owner')
UNION ALL
SELECT 'database ' || datname
FROM pg_database
WHERE datdba = (SELECT oid FROM pg_roles WHERE rolname = :'owner');
SQL
}

# snapshot ROLE: the exact row count of every table in schema public, then the
# full _sqlx_migrations rows in version order.
snapshot() {
	echo '== table row counts =='
	psql_as "$1" librepaper -f - <<'SQL'
SELECT c.relname || '|' || (xpath('/row/c/text()', query_to_xml(format('SELECT count(*) AS c FROM %I.%I', n.nspname, c.relname), false, true, '')))[1]::text
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p')
ORDER BY c.relname;
SQL
	echo '== _sqlx_migrations =='
	psql_as "$1" librepaper -f - <<'SQL'
SELECT * FROM _sqlx_migrations ORDER BY version;
SQL
}

# The whole script lives in main(). Over ssh the script is bash's standard
# input, and docker compose exec forwards standard input to the container: a
# bare exec would swallow the rest of the script. bash parses main() completely
# before it runs, and the first thing main() does is point standard input at
# /dev/null.
main() {
	exec </dev/null
	umask 077
	trap 'printf "convert-database: failed at line %s: %s\n" "$LINENO" "$BASH_COMMAND" >&2' ERR

	local convert_dir=${LIBREPAPER_CONVERT_DIR:-$HOME/librepaper-convert}
	local tarball=$convert_dir/host-files-before.tar.gz
	local dump=$convert_dir/librepaper.dump
	local before=$convert_dir/before.txt
	local after=$convert_dir/after.txt
	local parent file services config running
	parent=$(dirname "$PWD")

	say "working in $PWD, artifacts in $convert_dir"
	[[ -f $tarball ]] || die "$tarball is missing: take the host files tarball first, then retry"
	tar tzf "$tarball" >/dev/null || die "$tarball is not a readable archive"
	for file in compose.yaml librepaper.toml postgres/init.sql; do
		[[ -f $file ]] || die "$file is missing: sync the new kit into $PWD first"
	done
	services=$(docker compose config --services)
	if grep -qx migrate <<<"$services"; then
		die "compose.yaml still defines the migrate service: sync the new kit first"
	fi
	config=$(docker compose config)
	if ! grep -q 'network_mode: none' <<<"$config"; then
		die "compose.yaml does not give postgres network_mode: none: sync the new kit first"
	fi
	running=$(docker compose ps --status running --services)
	if ! grep -qx postgres <<<"$running"; then
		die "the old postgres service is not running under this Compose project"
	fi

	say "preconditions: the role catalog is exactly the one the legacy upgrade left"
	local expected found missing unexpected
	expected='librepaper super nologin
librepaper_app nosuper login
librepaper_backup nosuper login
librepaper_bootstrap super login
librepaper_metrics nosuper login
librepaper_owner nosuper login'
	found=$(psql_as librepaper_bootstrap librepaper -f - <<'SQL'
SELECT rolname || ' ' || CASE WHEN rolsuper THEN 'super' ELSE 'nosuper' END || ' ' || CASE WHEN rolcanlogin THEN 'login' ELSE 'nologin' END
FROM pg_roles
WHERE oid = 10 OR rolname LIKE 'librepaper%';
SQL
)
	missing=$(LC_ALL=C comm -23 <(sort_lines "$expected") <(sort_lines "$found"))
	unexpected=$(LC_ALL=C comm -13 <(sort_lines "$expected") <(sort_lines "$found"))
	if [[ -n $missing || -n $unexpected ]]; then
		die "the role catalog differs. Missing (rolname, superuser, login): ${missing:-none}. Unexpected: ${unexpected:-none}"
	fi
	local postgres_roles
	postgres_roles=$(psql_as librepaper_bootstrap librepaper -c "SELECT count(*) FROM pg_roles WHERE rolname = 'postgres'")
	if [[ $postgres_roles != 0 ]]; then
		die "a role named postgres already exists: renaming the initdb superuser would collide with it"
	fi

	say "preconditions: librepaper_app and librepaper_backup own nothing, librepaper_bootstrap owns nothing"
	local role database owned
	for role in librepaper_app librepaper_backup librepaper_bootstrap; do
		for database in librepaper postgres; do
			owned=$(owned_objects "$role" "$database")
			if [[ -n $owned ]]; then
				die "$role owns objects in database $database, DROP OWNED would drop them:"$'\n'"$owned"
			fi
		done
	done

	say "stopping librepaper, backup and postgres-exporter: this script is now the only client"
	docker compose --profile monitoring stop librepaper backup postgres-exporter

	say "dumping the librepaper database to $dump (the fallback)"
	docker compose exec -T postgres pg_dump -U librepaper_owner -d librepaper -Fc >"$dump.partial"
	[[ -s $dump.partial ]] || die "the dump is empty"
	[[ $(head -c 5 "$dump.partial") == PGDMP ]] || die "the dump is not a pg_dump custom format archive"
	mv -f "$dump.partial" "$dump"

	say "recording row counts and _sqlx_migrations as librepaper_owner in $before"
	snapshot librepaper_owner >"$before"

	say "first SQL block, one transaction (psql -1), as librepaper_bootstrap"
	if [[ ${LIBREPAPER_CONVERT_INJECT_FAILURE:-} == 1 ]]; then
		say "LIBREPAPER_CONVERT_INJECT_FAILURE=1: a failing statement is appended, this block must roll back"
	fi
	if ! {
		cat <<'SQL'
ALTER ROLE librepaper RENAME TO postgres;
ALTER ROLE postgres LOGIN;
ALTER ROLE librepaper_owner RENAME TO librepaper;
ALTER ROLE librepaper INHERIT PASSWORD NULL;
DROP OWNED BY librepaper_app, librepaper_backup;
DROP ROLE librepaper_app, librepaper_backup;
ALTER ROLE librepaper_metrics PASSWORD NULL;
GRANT CONNECT ON DATABASE librepaper TO PUBLIC;
SQL
		if [[ ${LIBREPAPER_CONVERT_INJECT_FAILURE:-} == 1 ]]; then
			echo 'SELECT 1/0;'
		fi
	} | psql_as librepaper_bootstrap librepaper -1 -f -; then
		die "the first SQL block failed and rolled back, the database is unchanged. App and backup are stopped. To go back: tar xzf $tarball -C $parent && docker compose up -d"
	fi

	say "second SQL block, one transaction (psql -1), reconnected as postgres: a role cannot drop itself"
	if ! psql_as postgres librepaper -1 -f - <<'SQL'
DROP OWNED BY librepaper_bootstrap;
DROP ROLE librepaper_bootstrap;
SQL
	then
		die "the roles are converted but librepaper_bootstrap remains as an extra superuser. The stack still works: drop the role by hand as postgres, then run docker compose up -d --wait"
	fi

	# Postgres first, with the app still stopped: the app's own startup work
	# (sweeps, lease rows) must not be able to move the counts being compared.
	say "recreating postgres without a network and with the socket volume"
	docker compose up -d --wait --wait-timeout 180 postgres

	say "recording row counts and _sqlx_migrations as librepaper in $after"
	snapshot librepaper >"$after"
	if ! diff -u "$before" "$after" >&2; then
		die "row counts or _sqlx_migrations changed during the conversion (diff above). The app stays stopped; the dump is $dump"
	fi

	say "starting the converted stack: the app validates the schema"
	docker compose up -d --wait --wait-timeout 180

	say "deleting the old kit files by name"
	rm -f setup postgres/roles.sql postgres/init-roles.sh .setup-state.json monitoring/grafana-entrypoint.sh \
		compose.external-db.yaml compose.local-binary.yaml compose.local-build.yaml compose.monitoring.yaml compose.production.yaml \
		secrets/database_app_url secrets/database_owner_url secrets/database_backup_url secrets/database_metrics_url secrets/database_metrics_uri secrets/database_metrics_user \
		secrets/database_app_password secrets/database_owner_password secrets/database_backup_password secrets/database_metrics_password \
		secrets/postgres_bootstrap_password secrets/grafana_admin_password

	local tables migrations
	tables=$(awk '/^== _sqlx_migrations/ {exit} !/^==/ {n++} END {print n+0}' "$after")
	migrations=$(awk 'm {n++} /^== _sqlx_migrations/ {m=1} END {print n+0}' "$after")
	say "converted: $tables tables and $migrations migrations unchanged, stack healthy; delete $convert_dir after a week"
}

main "$@"
