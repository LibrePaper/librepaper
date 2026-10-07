#!/usr/bin/env bash
set -Eeuo pipefail

# Boot the shipped Compose kit in two throwaway Compose projects.
#
#   local    the kit as extracted: its own PostgreSQL on a socket with no
#            network, the app migrating at start-up, /ready answering from
#            inside the app container, and an app role that is not a superuser
#   managed  compose.managed-db.yaml as compose.override.yaml, with a disposable
#            TCP PostgreSQL standing in for a provider's, then the documented
#            upgrade (the kit extracted over the same directory without the two
#            config files) to show the override survives
#
# Requirements:
#   - docker with Compose 2.24.4 or later (the overrides use !reset and !override)
#   - web/dist built (make web) when the images are built from this checkout
#   - DEPLOY_SMOKE_APP_IMAGE and DEPLOY_SMOKE_BACKUP_IMAGE, optional: prebuilt
#     images to run instead of building the app and backup images from the
#     checkout; each one that is set skips its build
#   - DEPLOY_INSTALL_KEEP=1 keeps the temporary kit directories
#
# usage: tools/test/deploy/install.sh [local|managed|all]
#
# Only projects, volumes, networks, containers and images named with this run's
# random suffix are created or removed. Caddy publishes no host port here and
# the edge network gets a random private subnet, so the test does not collide
# with a running kit.

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
deploy="$root/deploy"
variant="${1:-all}"
case "$variant" in
local | managed | all) ;;
*) echo 'usage: tools/test/deploy/install.sh [local|managed|all]' >&2; exit 2 ;;
esac

fail() { echo "install: $*" >&2; exit 1; }

command -v docker >/dev/null || fail 'docker is required'
docker compose version >/dev/null 2>&1 || fail 'Docker Compose v2 is required'
app_prebuilt="${DEPLOY_SMOKE_APP_IMAGE:-}"
backup_prebuilt="${DEPLOY_SMOKE_BACKUP_IMAGE:-}"
for image in $app_prebuilt $backup_prebuilt; do
	docker image inspect "$image" >/dev/null 2>&1 || fail "prebuilt image is missing: $image"
done
if [[ -z "$app_prebuilt" || -z "$backup_prebuilt" ]]; then
	[[ -f "$root/web/dist/index.html" ]] || fail 'web/dist is missing: run make web, or set DEPLOY_SMOKE_APP_IMAGE and DEPLOY_SMOKE_BACKUP_IMAGE'
fi

# Nothing from the caller's shell may change which files or profiles Compose uses.
unset COMPOSE_FILE COMPOSE_PROFILES COMPOSE_PROJECT_NAME COMPOSE_ENV_FILES
export COMPOSE_DISABLE_ENV_FILE=1

. "$root/tools/test/postgres.sh"

work=$(mktemp -d "${TMPDIR:-/tmp}/librepaper-deploy-install.XXXXXX")
case "$work" in */librepaper-deploy-install.*) ;; *) echo 'unsafe temporary path' >&2; exit 2 ;; esac
suffix="${work##*.}"
suffix="${suffix,,}"
local_project="lp-install-local-$suffix"
managed_project="lp-install-managed-$suffix"
local_dir="$work/local"
managed_dir="$work/managed"
db_container="lp-install-db-$suffix"
db_network="lp-install-dbnet-$suffix"
app_image="${app_prebuilt:-librepaper-install-test:$suffix}"
backup_image="${backup_prebuilt:-librepaper-backup-install-test:$suffix}"
local_owned=0
managed_owned=0
db_network_owned=0

# The kit directory is the project directory, so Compose loads compose.yaml and
# compose.override.yaml from it on its own, as it does for an operator. The
# managed variant lists its files in COMPOSE_FILE, which turns that loading off.
local_compose() { (cd "$local_dir" && docker compose -p "$local_project" "$@"); }
managed_compose() {
	(cd "$managed_dir" &&
		COMPOSE_FILE=compose.yaml:compose.override.yaml:compose.test.yaml \
			docker compose -p "$managed_project" "$@")
}

cleanup() {
	local status=$?
	trap - EXIT
	if (( status != 0 )); then
		if (( local_owned )); then local_compose logs --no-color --tail=60 >&2 || true; fi
		if (( managed_owned )); then managed_compose logs --no-color --tail=60 >&2 || true; fi
	fi
	if (( local_owned )); then local_compose down --volumes --remove-orphans >/dev/null 2>&1 || true; fi
	if (( managed_owned )); then managed_compose down --volumes --remove-orphans >/dev/null 2>&1 || true; fi
	postgres_stop_disposable
	if (( db_network_owned )); then docker network rm "$db_network" >/dev/null 2>&1 || true; fi
	if [[ -z "$app_prebuilt" ]]; then docker image rm -f "$app_image" >/dev/null 2>&1 || true; fi
	if [[ -z "$backup_prebuilt" ]]; then docker image rm -f "$backup_image" >/dev/null 2>&1 || true; fi
	if [[ "${DEPLOY_INSTALL_KEEP:-0}" == 1 || $status != 0 ]]; then
		echo "install: kit directories preserved at $work" >&2
	else
		rm -rf -- "$work"
	fi
	exit "$status"
}
trap cleanup EXIT

assert_project_unused() {
	local project="$1" volume
	if [[ -n "$(docker ps -aq --filter "label=com.docker.compose.project=$project")" ]]; then
		fail "refusing to reuse existing Compose project $project"
	fi
	for volume in "${project}_postgres" "${project}_data"; do
		if docker volume inspect "$volume" >/dev/null 2>&1; then
			fail "refusing to reuse existing Docker volume $volume"
		fi
	done
	if [[ -n "$(docker network ls -q --filter "label=com.docker.compose.project=$project")" ]]; then
		fail "refusing to reuse existing Compose network for $project"
	fi
}

random_edge_subnet() {
	local high low
	high=$(od -An -N1 -tu1 /dev/urandom | tr -d ' ')
	low=$(od -An -N1 -tu1 /dev/urandom | tr -d ' ')
	printf '10.%s.%s.0/24' "$((240 + high % 16))" "$((1 + low % 254))"
}

# The kit as the release archive holds it: deploy/ without the image build
# inputs and without anything an operator keeps beside it.
stage_kit() {
	local dest="$1"
	mkdir -p "$dest"
	cp -R "$deploy/." "$dest/"
	rm -rf -- "$dest/Dockerfile" "$dest/backup" "$dest/compose.override.yaml" \
		"$dest/site" "$dest/caddy/local.d" \
		"$dest"/*.candidate*
	chmod -R go+rX "$dest"
}

# Rewrite one top-level key of the kit's librepaper.toml and prove it took.
set_toml_line() {
	local file="$1" key="$2" value="$3"
	sed -E "s|^$key = .*|$key = $value|" "$file" >"$file.new"
	mv "$file.new" "$file"
	chmod 0644 "$file"
	grep -qxF "$key = $value" "$file" || fail "could not set $key in the kit's librepaper.toml"
}

set_origins() {
	local toml="$1"
	set_toml_line "$toml" app '"http://localhost"'
	set_toml_line "$toml" docs '"http://docs.localhost"'
	# Uncomment and set the admin origin line
	sed -E 's/^# admin = .*$/admin = "http:\/\/admin.localhost"/' "$toml" >"$toml.new"
	mv "$toml.new" "$toml"
	chmod 0644 "$toml"
	grep -qxF 'admin = "http://admin.localhost"' "$toml" || fail "could not set admin in the kit's librepaper.toml"
	# Append the [admin] section with password
	echo '[admin]' >>"$toml"
	echo 'password = "deploy-install-test-password"' >>"$toml"
}

service_override() {
	local service="$1" image="$2" target="$3" prebuilt="$4"
	printf '  %s:\n    image: %s\n' "$service" "$image"
	if [[ -z "$prebuilt" ]]; then
		cat <<YAML
    build:
      context: $root
      dockerfile: deploy/Dockerfile
      target: $target
      args:
        SOURCE: checkout
YAML
	fi
}

# Run the checkout's images, publish no host port and use a private subnet.
# With a network argument the app and backup also join it, which is how they
# reach the disposable PostgreSQL.
write_test_override() {
	local path="$1" subnet="$2" dbnet="${3:-}"
	{
		echo 'services:'
		service_override librepaper "$app_image" app "$app_prebuilt"
		if [[ -n "$dbnet" ]]; then echo '    networks: [dbnet]'; fi
		service_override backup "$backup_image" backup "$backup_prebuilt"
		if [[ -n "$dbnet" ]]; then echo '    networks: [dbnet]'; fi
		cat <<YAML
  caddy:
    ports: !reset []
networks:
  edge:
    ipam:
      config: !override
        - subnet: $subnet
YAML
		if [[ -n "$dbnet" ]]; then
			cat <<YAML
  dbnet:
    external: true
    name: $dbnet
YAML
		fi
	} >"$path"
	chmod 0644 "$path"
}

build_images() {
	local compose="$1" services=()
	if [[ -z "$app_prebuilt" ]]; then services+=(librepaper); fi
	if [[ -z "$backup_prebuilt" ]]; then services+=(backup); fi
	if (( ${#services[@]} )); then "$compose" build "${services[@]}"; fi
}

# /ready is asked from inside the app container: no host port, no DNS, no TLS.
app_ready() {
	local compose="$1" _
	for _ in {1..30}; do
		if "$compose" exec -T librepaper wget -qO- http://127.0.0.1:8080/ready >/dev/null 2>&1; then
			return 0
		fi
		sleep 2
	done
	fail "the app did not answer /ready ($compose)"
}

# Admin graphs endpoint must return twelve series with basic auth.
admin_graphs_answer() {
	local compose="$1" count
	# With credentials, the endpoint returns data with twelve series.
	count=$("$compose" exec -T librepaper wget -qO- --header 'Host: admin.localhost' 'http://admin:deploy-install-test-password@127.0.0.1:8080/data?points=10' | grep -o '"name":' | wc -l)
	[[ "$count" == 12 ]] || fail "admin graphs endpoint returned $count series, expected 12 ($compose)"
	# Without credentials, it must return 401.
	local exit_code
	exit_code=0
	"$compose" exec -T librepaper wget -qO- --header 'Host: admin.localhost' 'http://127.0.0.1:8080/data?points=10' >/dev/null 2>&1 || exit_code=$?
	if [[ $exit_code -eq 0 ]]; then
		fail "admin graphs endpoint without credentials should fail ($compose)"
	fi
	echo "admin graphs: authenticated endpoint returns twelve series and unauthenticated returns 401"
}

# The override is in effect when neither the configuration nor the containers
# have a postgres service. The app and the backup sidecar must still be there.
# Output is captured first: grep -q exiting early would trip pipefail.
assert_no_local_postgres() {
	local when="$1" configured running
	configured=$(managed_compose config --services)
	if grep -qx postgres <<<"$configured"; then
		fail "the configuration has a postgres service $when"
	fi
	running=$(managed_compose ps --all --services)
	grep -qx librepaper <<<"$running" || fail "the app is not a service $when"
	grep -qx backup <<<"$running" || fail "the backup sidecar is not a service $when"
	if grep -qx postgres <<<"$running"; then fail "postgres is a running service $when"; fi
}

run_local() {
	echo '== local database'
	assert_project_unused "$local_project"
	local_owned=1
	stage_kit "$local_dir"
	set_origins "$local_dir/librepaper.toml"
	write_test_override "$local_dir/compose.override.yaml" "$(random_edge_subnet)"
	build_images local_compose
	local_compose up -d --wait --wait-timeout 240
	app_ready local_compose
	admin_graphs_answer local_compose

	local migrations superuser mode
	migrations=$(local_compose exec -T postgres psql -U librepaper -d librepaper -XAtc 'select count(*) from _sqlx_migrations')
	[[ "$migrations" =~ ^[0-9]+$ ]] && (( migrations > 0 )) || fail "the app applied no migrations (count: $migrations)"
	superuser=$(local_compose exec -T postgres psql -U postgres -XAtc "select rolsuper from pg_roles where rolname='librepaper'")
	[[ "$superuser" == f ]] || fail "the librepaper role must not be a superuser (rolsuper: $superuser)"
	mode=$(docker inspect -f '{{.HostConfig.NetworkMode}}' "$(local_compose ps -q postgres)")
	[[ "$mode" == none ]] || fail "postgres must run without a network (mode: $mode)"
	echo "local database: ready, $migrations migrations, librepaper is not a superuser, postgres has no network"

	local_compose down --volumes --remove-orphans >/dev/null 2>&1 || true
}

run_managed() {
	echo '== managed database'
	assert_project_unused "$managed_project"
	managed_owned=1

	# A provider's database stand-in: private TCP, a password, and an owner role
	# that is not a superuser. The container joins a network made for this run so
	# the app can reach it by name; its published loopback port is not used.
	local db_password=managed-test-password migrations
	docker network create "$db_network" >/dev/null
	db_network_owned=1
	postgres_start_disposable "$db_container" "$db_password"
	docker network connect "$db_network" "$POSTGRES_CONTAINER"
	docker exec "$POSTGRES_CONTAINER" psql -U postgres -X -v ON_ERROR_STOP=1 \
		-c "CREATE ROLE librepaper LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD '$db_password'" \
		-c 'CREATE DATABASE librepaper OWNER librepaper' >/dev/null

	stage_kit "$managed_dir"
	set_origins "$managed_dir/librepaper.toml"
	# sslmode=disable is for this test only: the disposable server has no TLS and
	# is reachable only over a Docker network made for this run. A real managed
	# database is configured with sslmode=verify-full.
	set_toml_line "$managed_dir/librepaper.toml" database_url \
		"\"postgresql://librepaper:$db_password@$db_container:5432/librepaper?sslmode=disable\""
	cp "$managed_dir/compose.managed-db.yaml" "$managed_dir/compose.override.yaml"
	write_test_override "$managed_dir/compose.test.yaml" "$(random_edge_subnet)" "$db_network"

	build_images managed_compose
	managed_compose up -d --wait --wait-timeout 240
	app_ready managed_compose
	admin_graphs_answer managed_compose
	assert_no_local_postgres "after the first start"
	migrations=$(docker exec "$POSTGRES_CONTAINER" psql -U librepaper -d librepaper -XAtc 'select count(*) from _sqlx_migrations')
	[[ "$migrations" =~ ^[0-9]+$ ]] && (( migrations > 0 )) || fail "the app migrated nothing on the managed database (count: $migrations)"
	echo "managed database: ready, $migrations migrations applied over TCP, no postgres service"

	# The documented upgrade: the new kit is extracted over the directory, minus
	# the two config files the operator edits. The old compose.yaml is marked so
	# the test can see it was replaced.
	echo '== upgrade over the managed install'
	printf '\n# stale\n' >>"$managed_dir/compose.yaml"
	(cd "$managed_dir" && sha256sum compose.override.yaml compose.test.yaml librepaper.toml resticprofile.toml >"$work/operator.sums")
	stage_kit "$work/archive"
	rm -f -- "$work/archive/librepaper.toml" "$work/archive/resticprofile.toml"
	cp -R "$work/archive/." "$managed_dir/"
	if grep -q '^# stale$' "$managed_dir/compose.yaml"; then fail 'the extracted compose.yaml did not replace the old one'; fi
	(cd "$managed_dir" && sha256sum --check --quiet "$work/operator.sums") ||
		fail 'the upgrade changed a file the operator owns'
	managed_compose up -d --wait --wait-timeout 240
	app_ready managed_compose
	assert_no_local_postgres "after the upgrade"
	echo 'upgrade: the override and both config files survived, the app still answers /ready'

	managed_compose down --volumes --remove-orphans >/dev/null 2>&1 || true
}

case "$variant" in
local) run_local ;;
managed) run_managed ;;
all) run_local; run_managed ;;
esac
echo 'deploy install: passed'
