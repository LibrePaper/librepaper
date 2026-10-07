#!/usr/bin/env bash
# TEMPORARY. Rehearsal of tools/deploy/convert-database.sh, the one-time
# conversion of the production cluster from the scoped-role kit to the
# socket-trust kit. Delete this file, convert-database.sh, the
# convert-database subcommand of tools/deploy/production and the deploy-convert
# suite entry once production is converted (SPEC-simple-deploy-kit.md).
#
# Fixture: the pre-cutover tree (git archive ba9fc74c deploy) is brought to the
# state production is in: a socket-trust cluster with POSTGRES_USER=librepaper
# and migrations applied by the superuser, `setup upgrade` to the scoped roles,
# the app image on top, one account row. The new kit from this checkout's
# deploy/ is then synced in and tools/deploy/convert-database.sh is streamed to
# `bash -s` exactly as `production convert-database` streams it over ssh.
#
# Scenario a: the conversion succeeds. The role catalog, database owner and
# effective privileges, public schema owner and effective privileges, default
# ACLs and object owners are diffed against a fresh install of the same new kit;
# row counts, _sqlx_migrations, session.key, /ready and a second `up -d` follow.
# Scenario b: LIBREPAPER_CONVERT_INJECT_FAILURE=1 makes the first SQL block fail;
# the role catalog must be byte-identical afterwards and, after the host files
# tarball is restored, the old stack must answer /ready.
#
# Needs docker with Compose v2, git with the ba9fc74c history, tar, diff, cmp,
# sed and sort. python3 and openssl are needed only for the legacy `setup`
# fixture; wget must exist inside the app image (the image's own health check
# uses it). Network: pulls the app image (DEPLOY_SMOKE_APP_IMAGE, default
# ghcr.io/librepaper/librepaper:v0.0.21), postgres, and whatever the kit's
# compose.yaml names for the backup sidecar. Caddy is disabled by the override
# so nothing binds ports 80 or 443.
set -Eeuo pipefail
trap 'echo "convert rehearsal: line $LINENO: $BASH_COMMAND" >&2' ERR

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
kit="$root/deploy"
convert_script="$root/tools/deploy/convert-database.sh"
legacy_ref=ba9fc74c
app_image="${DEPLOY_SMOKE_APP_IMAGE:-ghcr.io/librepaper/librepaper:v0.0.21}"
postgres_image="${DEPLOY_SMOKE_POSTGRES_IMAGE:-postgres:17.11-alpine}"
account_id=7fc7d2d6-4522-4d1f-9cb2-c5d52bbdc3b8

for tool in docker git tar diff cmp sed sort od python3 openssl; do
	command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
done
docker compose version >/dev/null 2>&1 || { echo 'docker compose v2 is required' >&2; exit 1; }
git -C "$root" cat-file -e "$legacy_ref^{commit}" 2>/dev/null || {
	echo "git history is missing $legacy_ref, the last commit with the scoped-role kit" >&2
	exit 1
}
for item in compose.yaml compose.managed-db.yaml librepaper.toml resticprofile.toml \
	README.md postgres/init.sql caddy/Caddyfile; do
	[[ -e $kit/$item ]] || {
		echo "the new kit is not in this checkout: deploy/$item is missing. Run this after the kit branch is integrated" >&2
		exit 1
	}
done
for image in "$app_image" "$postgres_image"; do
	docker image inspect "$image" >/dev/null 2>&1 || docker pull -q "$image" >/dev/null || {
		echo "cannot find or pull $image" >&2
		exit 1
	}
done
# Compose settings from the caller's shell would redirect every command below.
unset COMPOSE_FILE COMPOSE_PROFILES COMPOSE_PROJECT_NAME COMPOSE_ENV_FILES COMPOSE_PATH_SEPARATOR

work=$(mktemp -d /tmp/librepaper-deploy-convert.XXXXXX)
case "$work" in /tmp/librepaper-deploy-convert.*) ;; *) echo 'unsafe temporary path' >&2; exit 2;; esac
suffix="${work##*.}"
suffix="${suffix,,}"
projects=()
project_dirs=()
used_subnets=''

pass() { printf 'PASS %s\n' "$*"; }
fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# remove_project PROJECT: everything Compose labelled with exactly this project.
remove_project() {
	local project=$1 ids
	ids=$(docker ps -aq --filter "label=com.docker.compose.project=$project")
	# shellcheck disable=SC2086
	[[ -z $ids ]] || docker rm -f $ids >/dev/null 2>&1 || true
	docker rm -f "${project}-binary" >/dev/null 2>&1 || true
	ids=$(docker network ls -q --filter "label=com.docker.compose.project=$project")
	# shellcheck disable=SC2086
	[[ -z $ids ]] || docker network rm $ids >/dev/null 2>&1 || true
	ids=$(docker volume ls -q --filter "label=com.docker.compose.project=$project")
	# shellcheck disable=SC2086
	[[ -z $ids ]] || docker volume rm $ids >/dev/null 2>&1 || true
	docker volume rm "${project}_postgres" "${project}_data" >/dev/null 2>&1 || true
}

# teardown DIR PROJECT: only ever called for projects this script created.
teardown() {
	local dir=$1 project=$2
	if [[ -d $dir ]]; then
		(cd "$dir" && docker compose --project-name "$project" down --volumes --remove-orphans) >/dev/null 2>&1 || true
	fi
	remove_project "$project"
}

cleanup() {
	local status=$? index
	trap - EXIT
	for ((index = 0; index < ${#projects[@]}; index++)); do
		teardown "${project_dirs[$index]}" "${projects[$index]}"
	done
	if [[ ${DEPLOY_CONVERT_KEEP:-0} == 1 || $status != 0 ]]; then
		echo "convert rehearsal files preserved at $work" >&2
	else
		rm -rf -- "$work" || true
	fi
	exit "$status"
}
trap cleanup EXIT

assert_project_unused() {
	local project=$1 volume
	if [[ -n "$(docker ps -aq --filter "label=com.docker.compose.project=$project")" ]]; then
		echo "refusing to reuse existing Compose project $project" >&2
		exit 2
	fi
	for volume in "${project}_postgres" "${project}_data"; do
		if docker volume inspect "$volume" >/dev/null 2>&1; then
			echo "refusing to reuse existing Docker volume $volume" >&2
			exit 2
		fi
	done
	if [[ -n "$(docker network ls -q --filter "label=com.docker.compose.project=$project")" ]]; then
		echo "refusing to reuse existing Compose network for $project" >&2
		exit 2
	fi
}

# pick_subnet sets FX_SUBNET to a random /24 that no fixture here has used. The
# kit's fixed 172.29.0.0/16 would collide between concurrent projects.
pick_subnet() {
	local high low
	while :; do
		high=$(od -An -N1 -tu1 /dev/urandom | tr -d ' ')
		low=$(od -An -N1 -tu1 /dev/urandom | tr -d ' ')
		FX_SUBNET="10.$((240 + high % 16)).$((1 + low % 254)).0/24"
		case " $used_subnets " in
		*" $FX_SUBNET "*) continue ;;
		esac
		used_subnets+=" $FX_SUBNET"
		return 0
	done
}

# set_fixture NAME sets the FX_* variables for one disposable project and
# registers it for cleanup before anything is created.
set_fixture() {
	FX_NAME=$1
	FX_BASE="$work/$1"
	FX_DIR="$FX_BASE/librepaper"
	FX_CONV="$FX_BASE/convert"
	FX_PROJECT="lp-convert-$1-$suffix"
	FX_HOST="app-$1-$suffix.example.test"
	pick_subnet
	assert_project_unused "$FX_PROJECT"
	projects+=("$FX_PROJECT")
	project_dirs+=("$FX_DIR")
	mkdir -p "$FX_BASE" "$FX_CONV"
}

# kit_compose DIR PROJECT ARGS...: docker compose in a kit directory, as the
# operator runs it, but with the disposable project name forced.
kit_compose() {
	local dir=$1 project=$2
	shift 2
	(cd "$dir" && docker compose --project-name "$project" "$@")
}

# sql DIR PROJECT ROLE ARGS...: psql in the postgres container over its socket.
sql() {
	local dir=$1 project=$2 role=$3
	shift 3
	kit_compose "$dir" "$project" exec -T postgres psql -XAtq -v ON_ERROR_STOP=1 -U "$role" -d librepaper "$@"
}

app_ready() {
	kit_compose "$1" "$2" exec -T librepaper wget -qO- http://127.0.0.1:8080/ready >/dev/null 2>&1
}

wait_ready() {
	local dir=$1 project=$2 attempt
	for attempt in $(seq 1 180); do
		if app_ready "$dir" "$project"; then
			return 0
		fi
		sleep 1
	done
	echo "the app did not answer /ready within 180 seconds" >&2
	kit_compose "$dir" "$project" logs --no-color --tail 40 >&2 || true
	return 1
}

# The project name must resolve from the environment alone, the way the
# streamed script's own `docker compose` calls resolve it: the kit says
# `name: librepaper`, and a wrong answer here would touch a real project.
assert_project_name() {
	local dir=$1 expected=$2 resolved
	resolved=$(cd "$dir" && COMPOSE_PROJECT_NAME="$expected" docker compose config | sed -n 's/^name: //p')
	[[ $resolved == "$expected" ]] || {
		echo "Compose resolved project name '$resolved' instead of '$expected'; refusing to continue" >&2
		exit 2
	}
}

read_session_key() {
	docker run --rm --entrypoint cat -v "$1:/data:ro" "$postgres_image" /data/secrets/session.key
}

# write_config PATH HOST old|new. The old config reads the owner URL from a
# secret file and leaves migration to the one-shot service; the new one names
# the socket and lets the server migrate.
write_config() {
	local path=$1 host=$2 kind=$3 database_url migrate=''
	if [[ $kind == old ]]; then
		database_url='{ file = "/run/secrets/database_url" }'
		migrate='migrate = false'
	else
		database_url='"postgresql:///librepaper?host=/var/run/postgresql&user=librepaper"'
	fi
	rm -f -- "$path"
	cat >"$path" <<TOML
[origins]
app = "http://$host"
docs = "http://docs.$host"

[storage]
directory = "/var/lib/librepaper"
database_url = $database_url

[server]
address = "0.0.0.0:8080"
$migrate

[access]
publishers = ["any"]
commenters = ["any"]
TOML
	if [[ $kind == old ]]; then chmod 0444 "$path"; else chmod 0644 "$path"; fi
}

# write_override PATH old|new SUBNET. Test image, a random edge subnet, and no
# Caddy so nothing binds ports 80 or 443. Only the old kit has a migrate service,
# and defining it here against the new kit would create a second app.
write_override() {
	local path=$1 kind=$2 subnet=$3
	{
		echo 'services:'
		if [[ $kind == old ]]; then
			printf '  migrate:\n    image: %s\n' "$app_image"
		fi
		cat <<YAML
  librepaper:
    image: $app_image
  caddy:
    profiles: [unused]
YAML
		if [[ $kind == old ]]; then
			printf '  backup:\n    profiles: [unused]\n'
		fi
		cat <<YAML
networks:
  edge:
    ipam:
      config: !override
        - subnet: $subnet
YAML
	} >"$path"
}

# build_legacy_fixture NAME: the production-shaped cluster. Leaves FX_* set,
# the old stack running and host-files-before.tar.gz in FX_CONV.
build_legacy_fixture() {
	set_fixture "$1"
	local legacy_yaml="$FX_BASE/legacy-fixture.yaml"
	local postgres_volume="${FX_PROJECT}_postgres" data_volume="${FX_PROJECT}_data"
	local seed_container="${FX_PROJECT}-binary" postgres_container
	echo "== $FX_NAME: building the production-shaped fixture from $legacy_ref =="

	git -C "$root" archive "$legacy_ref" deploy | tar -x -C "$FX_BASE"
	mv "$FX_BASE/deploy" "$FX_DIR"
	write_config "$FX_DIR/librepaper.toml" "$FX_HOST" old

	# An old socket-only deployment under the project's volume names, which
	# `setup upgrade` then converts. The postgres service cannot join a network.
	cat >"$legacy_yaml" <<YAML
services:
  postgres:
    image: $postgres_image
    network_mode: none
    environment:
      POSTGRES_DB: librepaper
      POSTGRES_USER: librepaper
      POSTGRES_PASSWORD: legacy-disposable-password
      POSTGRES_INITDB_ARGS: --auth-host=trust --auth-local=trust
    volumes:
      - postgres:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -q -h 127.0.0.1 -U librepaper -d librepaper"]
      interval: 2s
      timeout: 2s
      retries: 30
  seed-data:
    image: $app_image
    user: "0:0"
    entrypoint: ["/bin/sh", "-c"]
    command: ["mkdir -p /data/secrets && cp /session.key /data/secrets/session.key && chown -R 10001:65534 /data && chmod 0700 /data /data/secrets && chmod 0600 /data/secrets/session.key"]
    volumes:
      - data:/data
      - $FX_BASE/legacy-session.key:/session.key:ro
volumes:
  postgres:
    name: $postgres_volume
  data:
    name: $data_volume
YAML
	openssl rand -hex 32 >"$FX_BASE/legacy-session.key"
	chmod 0600 "$legacy_yaml" "$FX_BASE/legacy-session.key"
	local fixture_compose=(docker compose --project-directory "$FX_BASE" --project-name "$FX_PROJECT" -f "$legacy_yaml")
	"${fixture_compose[@]}" up -d --wait postgres
	"${fixture_compose[@]}" run --rm seed-data

	# Migrate as the superuser, as the legacy kit did, so the objects start out
	# owned by the initdb role and `setup upgrade` has to hand them over.
	docker create --name "$seed_container" "$app_image" >/dev/null
	docker cp "$seed_container:/usr/local/bin/librepaper" "$FX_BASE/librepaper-binary"
	docker rm "$seed_container" >/dev/null
	postgres_container=$("${fixture_compose[@]}" ps -q postgres)
	docker exec "$postgres_container" mkdir -p /tmp/librepaper-upgrade-seed
	docker cp "$FX_BASE/librepaper-binary" "$postgres_container:/tmp/librepaper"
	cat >"$FX_BASE/legacy-migrate.toml" <<'TOML'
[storage]
directory = "/tmp"
database_url = "postgresql://librepaper:legacy-disposable-password@127.0.0.1:5432/librepaper"
TOML
	docker cp "$FX_BASE/legacy-migrate.toml" "$postgres_container:/tmp/librepaper-upgrade-seed/config.toml"
	docker exec "$postgres_container" chmod 0755 /tmp/librepaper
	docker exec "$postgres_container" /tmp/librepaper admin migrate --config /tmp/librepaper-upgrade-seed/config.toml
	docker exec "$postgres_container" psql -X -v ON_ERROR_STOP=1 -U librepaper -d librepaper -c \
		"INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status) VALUES ('$account_id','registered','github','github:legacy-upgrade-smoke','legacy-smoke','Legacy Upgrade Smoke','active')" >/dev/null

	cat >"$FX_DIR/.env" <<ENV
COMPOSE_PROJECT_NAME=$FX_PROJECT
COMPOSE_FILE=compose.yaml
LIBREPAPER_SECRETS_DIR=$FX_DIR/secrets
LIBREPAPER_CONFIG_FILE=$FX_DIR/librepaper.toml
ENV
	(cd "$FX_DIR" && python3 ./setup upgrade --yes --project "$FX_PROJECT")

	# setup leaves COMPOSE_FILE=compose.yaml; the override joins it the way
	# production's .env lists its overlays, so a plain `docker compose` finds it.
	sed 's/^COMPOSE_FILE=.*/COMPOSE_FILE=compose.yaml:compose.override.yaml/' "$FX_DIR/.env" >"$FX_DIR/.env.new"
	mv -f "$FX_DIR/.env.new" "$FX_DIR/.env"
	write_override "$FX_DIR/compose.override.yaml" old "$FX_SUBNET"
	assert_project_name "$FX_DIR" "$FX_PROJECT"
	kit_compose "$FX_DIR" "$FX_PROJECT" up -d postgres migrate librepaper
	wait_ready "$FX_DIR" "$FX_PROJECT" || fail "$FX_NAME: the old stack never answered /ready"

	read_session_key "$data_volume" >"$FX_BASE/session-before.key"
	cmp -s "$FX_BASE/legacy-session.key" "$FX_BASE/session-before.key" || fail "$FX_NAME: the fixture session.key is not the seeded one"
	sql "$FX_DIR" "$FX_PROJECT" librepaper_owner -c 'SELECT * FROM _sqlx_migrations ORDER BY version' >"$FX_BASE/migrations-before.txt"
	[[ -s $FX_BASE/migrations-before.txt ]] || fail "$FX_NAME: the fixture has no _sqlx_migrations rows"

	# The host files exactly as they are now, before the new kit lands.
	chmod 0700 "$FX_DIR"
	tar czf "$FX_CONV/host-files-before.tar.gz" -C "$FX_BASE" librepaper
}

# install_new_kit DIR HOST SUBNET PROJECT: the new kit files from this checkout
# plus the operator's files, without starting anything.
install_new_kit() {
	local dir=$1 host=$2 subnet=$3 project=$4 item
	mkdir -p "$dir/postgres" "$dir/caddy"
	for item in compose.yaml compose.managed-db.yaml resticprofile.toml README.md postgres/init.sql; do
		cp -f -- "$kit/$item" "$dir/$item"
	done
	cp -Rf -- "$kit/caddy/." "$dir/caddy/"
	write_config "$dir/librepaper.toml" "$host" new
	# Create placeholder files for prometheus.yaml, grafana.json, and monitoring/
	# so the conversion script can delete them
	printf '# placeholder for old kit\n' >"$dir/prometheus.yaml"
	printf '# placeholder for old kit\n' >"$dir/grafana.json"
	mkdir -p "$dir/monitoring"
	printf '# placeholder for old kit\n' >"$dir/monitoring/placeholder"
	write_override "$dir/compose.override.yaml" new "$subnet"
	assert_project_name "$dir" "$project"
}

# run_convert DIR PROJECT CONVERT_DIR [NAME=value...]: the host script as
# `production convert-database` runs it over ssh, as a stream on bash's stdin.
run_convert() {
	local dir=$1 project=$2 conv=$3
	shift 3
	(cd "$dir" && env COMPOSE_PROJECT_NAME="$project" LIBREPAPER_CONVERT_DIR="$conv" "$@" bash -s <"$convert_script")
}

# catalog_dump DIR PROJECT: the compared catalog, one tagged line per fact,
# sorted. Privileges are compared as effective privileges: the converted
# cluster keeps explicit ACL entries that a fresh one leaves NULL (and a
# CONNECT entry for librepaper_metrics on top of PUBLIC's), which say the same
# thing in a different form. Raw ACLs are printed separately as NOTE lines.
catalog_dump() {
	sql "$1" "$2" postgres -f - <<'SQL' | LC_ALL=C sort
SELECT 'role|' || concat_ws('|', r.rolname, r.rolsuper, r.rolcanlogin, r.rolcreatedb, r.rolcreaterole, r.rolinherit, a.rolpassword IS NULL, r.rolreplication, r.rolbypassrls, r.rolconnlimit)
FROM pg_roles r JOIN pg_authid a ON a.oid = r.oid
WHERE r.oid = 10 OR r.rolname LIKE 'librepaper%';

SELECT 'member|' || concat_ws('|', m.rolname, g.rolname, am.admin_option, am.inherit_option, am.set_option)
FROM pg_auth_members am
JOIN pg_roles m ON m.oid = am.member
JOIN pg_roles g ON g.oid = am.roleid
WHERE m.oid = 10 OR m.rolname LIKE 'librepaper%' OR g.rolname LIKE 'librepaper%';

SELECT 'database|' || d.datname || '|' || pg_get_userbyid(d.datdba)
FROM pg_database d;

SELECT 'dbpriv|' || r.rolname || '|' || p.priv || '|' || has_database_privilege(r.oid, 'librepaper'::text, p.priv)::text
FROM pg_roles r CROSS JOIN (VALUES ('CONNECT'), ('CREATE'), ('TEMPORARY')) AS p(priv)
WHERE r.oid = 10 OR r.rolname LIKE 'librepaper%';

SELECT 'dbpriv|PUBLIC|' || a.privilege_type
FROM pg_database d, aclexplode(coalesce(d.datacl, acldefault('d', d.datdba))) a
WHERE d.datname = 'librepaper' AND a.grantee = 0;

SELECT 'schema|' || n.nspname || '|' || pg_get_userbyid(n.nspowner)
FROM pg_namespace n
WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema';

SELECT 'schemapriv|' || r.rolname || '|' || p.priv || '|' || has_schema_privilege(r.oid, 'public'::text, p.priv)::text
FROM pg_roles r CROSS JOIN (VALUES ('USAGE'), ('CREATE')) AS p(priv)
WHERE r.oid = 10 OR r.rolname LIKE 'librepaper%';

SELECT 'schemapriv|PUBLIC|' || a.privilege_type
FROM pg_namespace n, aclexplode(coalesce(n.nspacl, acldefault('n', n.nspowner))) a
WHERE n.nspname = 'public' AND a.grantee = 0;

SELECT 'defacl|' || concat_ws('|', pg_get_userbyid(d.defaclrole), coalesce(n.nspname, ''), d.defaclobjtype::text, d.defaclacl::text)
FROM pg_default_acl d LEFT JOIN pg_namespace n ON n.oid = d.defaclnamespace;

SELECT 'relation|' || c.relkind::text || '|' || c.relname || '|' || pg_get_userbyid(c.relowner)
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public';

SELECT 'type|' || t.typname || '|' || pg_get_userbyid(t.typowner)
FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
WHERE n.nspname = 'public';

SELECT 'function|' || p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')|' || pg_get_userbyid(p.proowner)
FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
WHERE n.nspname = 'public';

SELECT 'extension|' || e.extname || '|' || pg_get_userbyid(e.extowner)
FROM pg_extension e;
SQL
}

# note_acls LABEL DIR PROJECT: the raw ACLs, for the reader, not compared.
note_acls() {
	local label=$1 dir=$2 project=$3 datacl nspacl
	datacl=$(sql "$dir" "$project" postgres -c "SELECT coalesce(datacl::text, 'NULL') FROM pg_database WHERE datname = 'librepaper'")
	nspacl=$(sql "$dir" "$project" postgres -c "SELECT coalesce(nspacl::text, 'NULL') FROM pg_namespace WHERE nspname = 'public'")
	echo "NOTE $label raw datacl: $datacl"
	echo "NOTE $label raw nspacl: $nspacl"
}

# raw_catalog DIR PROJECT: the role catalog as the legacy upgrade left it,
# raw and sorted, with password hashes reduced to md5 digests.
raw_catalog() {
	sql "$1" "$2" librepaper_bootstrap -f - <<'SQL' | LC_ALL=C sort
SELECT concat_ws('|', 'role'::text, a.oid, a.rolname, a.rolsuper, a.rolinherit, a.rolcreaterole, a.rolcreatedb, a.rolcanlogin, a.rolreplication, a.rolbypassrls, a.rolconnlimit, a.rolvaliduntil, md5(coalesce(a.rolpassword, '')))
FROM pg_authid a;

SELECT concat_ws('|', 'member'::text, am.roleid, am.member, am.grantor, am.admin_option, am.inherit_option, am.set_option)
FROM pg_auth_members am;

SELECT concat_ws('|', 'database'::text, d.datname, d.datdba, coalesce(d.datacl::text, 'NULL'))
FROM pg_database d;

SELECT concat_ws('|', 'schema'::text, n.nspname, n.nspowner, coalesce(n.nspacl::text, 'NULL'))
FROM pg_namespace n
WHERE n.nspname = 'public';

SELECT concat_ws('|', 'defacl'::text, d.defaclrole, d.defaclnamespace, d.defaclobjtype::text, d.defaclacl::text)
FROM pg_default_acl d;
SQL
}

# The only accepted catalog difference: the public schema is owned by the
# database owner directly after conversion and through pg_database_owner on a
# fresh install. Each side must show exactly its own form before it is folded.
fold_public_owner() {
	local file=$1 owner=$2
	grep -qx "schema|public|$owner" "$file" || fail "$file does not list the public schema owned by $owner"
	sed "s/^schema|public|$owner\$/schema|public|DATABASE OWNER/" "$file"
}

rehearse_conversion() {
	build_legacy_fixture a
	local dir=$FX_DIR project=$FX_PROJECT base=$FX_BASE conv=$FX_CONV
	install_new_kit "$dir" "$FX_HOST" "$FX_SUBNET" "$project"

	echo '== a: convert-database.sh on the production-shaped fixture =='
	if ! run_convert "$dir" "$project" "$conv" 2>&1 | tee "$base/convert.log"; then
		fail 'convert-database.sh failed on the production-shaped fixture'
	fi
	pass 'convert-database.sh exited 0'

	cmp -s "$conv/before.txt" "$conv/after.txt" || fail 'before.txt and after.txt differ'
	grep -Eq '^accounts\|[1-9][0-9]*$' "$conv/before.txt" || fail 'before.txt does not show the seeded account row'
	[[ $(sed -n '/^== _sqlx_migrations/,$p' "$conv/before.txt" | wc -l) -gt 1 ]] || fail 'before.txt has no _sqlx_migrations rows'
	[[ -s $conv/librepaper.dump ]] || fail 'the fallback dump is missing or empty'
	pass 'row counts and _sqlx_migrations are identical before and after (before.txt equals after.txt)'

	sql "$dir" "$project" librepaper -c 'SELECT * FROM _sqlx_migrations ORDER BY version' >"$base/migrations-after.txt"
	cmp -s "$base/migrations-before.txt" "$base/migrations-after.txt" || {
		diff -u "$base/migrations-before.txt" "$base/migrations-after.txt" >&2 || true
		fail '_sqlx_migrations in the converted database differs from the fixture'
	}
	pass '_sqlx_migrations in the converted database equals the fixture, row for row'

	[[ $(sql "$dir" "$project" librepaper -c "SELECT provider_subject FROM accounts WHERE id = '$account_id'") == github:legacy-upgrade-smoke ]] ||
		fail 'the seeded account is missing after conversion'
	pass 'the seeded account is readable as librepaper'

	read_session_key "${project}_data" >"$base/session-after.key"
	cmp -s "$base/session-before.key" "$base/session-after.key" || fail 'session.key changed'
	pass 'session.key bytes are unchanged'

	local container mode
	container=$(kit_compose "$dir" "$project" ps -q postgres)
	mode=$(docker inspect -f '{{.HostConfig.NetworkMode}}' "$container")
	[[ $mode == none ]] || fail "the converted postgres container has network mode $mode instead of none"
	pass 'the converted postgres container has no network'

	app_ready "$dir" "$project" || fail 'the converted app does not answer /ready'
	pass 'the converted app answers /ready'

	local gone kept
	for gone in setup postgres/roles.sql postgres/init-roles.sh .setup-state.json \
		compose.external-db.yaml compose.local-binary.yaml compose.local-build.yaml compose.monitoring.yaml compose.production.yaml \
		prometheus.yaml grafana.json monitoring \
		secrets/database_app_url secrets/database_owner_url secrets/database_backup_url secrets/database_metrics_url secrets/database_metrics_uri secrets/database_metrics_user \
		secrets/database_app_password secrets/database_owner_password secrets/database_backup_password secrets/database_metrics_password \
		secrets/postgres_bootstrap_password secrets/grafana_admin_password .env; do
		[[ ! -e $dir/$gone ]] || fail "$gone should have been deleted"
	done
	for kept in compose.yaml compose.override.yaml librepaper.toml postgres/init.sql \
		secrets/restic_password secrets/github_client_id; do
		[[ -e $dir/$kept ]] || fail "$kept should have been kept"
	done
	pass 'the old kit files are gone and the operator files remain'

	echo '== a: a fresh install of the same new kit, for the catalog diff =='
	build_fresh_install
	local fresh_dir=$FX_DIR fresh_project=$FX_PROJECT
	catalog_dump "$dir" "$project" >"$base/catalog-converted.txt"
	catalog_dump "$fresh_dir" "$fresh_project" >"$base/catalog-fresh.txt"
	fold_public_owner "$base/catalog-converted.txt" librepaper >"$base/catalog-converted.folded"
	fold_public_owner "$base/catalog-fresh.txt" pg_database_owner >"$base/catalog-fresh.folded"
	note_acls converted "$dir" "$project"
	note_acls fresh "$fresh_dir" "$fresh_project"
	[[ -s $base/catalog-fresh.folded ]] || fail 'the fresh catalog dump is empty'
	if ! diff -u "$base/catalog-fresh.folded" "$base/catalog-converted.folded"; then
		fail 'the converted catalog differs from a fresh install of the new kit (diff above: - fresh, + converted)'
	fi
	pass 'the converted catalog matches a fresh install: roles, database and public schema owners and privileges, default ACLs, object owners'
	teardown "$fresh_dir" "$fresh_project"

	kit_compose "$dir" "$project" up -d --wait --wait-timeout 180
	app_ready "$dir" "$project" || fail 'the app does not answer /ready after the second up -d'
	pass 'a second docker compose up -d --wait changes nothing and /ready still answers'
	teardown "$dir" "$project"
}

# build_fresh_install: the new kit on empty volumes, so init.sql runs and the
# app migrates as librepaper. Leaves FX_* set to the fresh project.
build_fresh_install() {
	set_fixture fresh
	mkdir -p "$FX_DIR"
	install_new_kit "$FX_DIR" "$FX_HOST" "$FX_SUBNET" "$FX_PROJECT"
	kit_compose "$FX_DIR" "$FX_PROJECT" up -d --wait --wait-timeout 180
	wait_ready "$FX_DIR" "$FX_PROJECT" || fail 'the fresh install never answered /ready'
}

rehearse_rollback() {
	build_legacy_fixture b
	local dir=$FX_DIR project=$FX_PROJECT base=$FX_BASE conv=$FX_CONV
	install_new_kit "$dir" "$FX_HOST" "$FX_SUBNET" "$project"
	raw_catalog "$dir" "$project" >"$base/catalog-before.txt"
	[[ -s $base/catalog-before.txt ]] || fail 'b: the role catalog snapshot is empty'

	echo '== b: convert-database.sh with an injected failure in the first SQL block =='
	if run_convert "$dir" "$project" "$conv" LIBREPAPER_CONVERT_INJECT_FAILURE=1 >"$base/convert.log" 2>&1; then
		cat "$base/convert.log"
		fail 'convert-database.sh exited 0 although the first SQL block was made to fail'
	fi
	cat "$base/convert.log"
	grep -q 'division by zero' "$base/convert.log" || fail 'the script stopped, but not at the injected statement'
	[[ -s $conv/librepaper.dump && -s $conv/before.txt && ! -e $conv/after.txt ]] ||
		fail 'the script did not stop inside the first SQL block'
	pass 'convert-database.sh exits non-zero when the first SQL block fails'

	raw_catalog "$dir" "$project" >"$base/catalog-after.txt"
	cmp -s "$base/catalog-before.txt" "$base/catalog-after.txt" || {
		diff -u "$base/catalog-before.txt" "$base/catalog-after.txt" >&2 || true
		fail 'the role catalog changed although the transaction should have rolled back'
	}
	pass 'the role catalog is byte-identical after the failed transaction'

	tar xzf "$conv/host-files-before.tar.gz" -C "$base"
	grep -q '^  migrate:' "$dir/compose.yaml" || fail 'the host files tarball did not restore the old compose.yaml'
	kit_compose "$dir" "$project" up -d
	wait_ready "$dir" "$project" || fail 'the old app does not answer /ready after the host files are restored'
	pass 'after restoring the host files, docker compose up -d brings the old stack back and /ready answers'
}

rehearse_conversion
rehearse_rollback
echo 'deploy-convert rehearsal passed'
