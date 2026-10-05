#!/usr/bin/env bash
set -Eeuo pipefail

# Exercise deploy/setup init and upgrade against isolated Compose projects.
# Both paths use the repository's actual PostgreSQL service and role scripts;
# only the application image is overridden with the prebuilt smoke image.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
deploy="$root/deploy"
app_image="${DEPLOY_SMOKE_APP_IMAGE:-librepaper-app-test:yb5p}"
postgres_image="${DEPLOY_SMOKE_POSTGRES_IMAGE:-postgres:17.11-alpine}"
command -v docker >/dev/null || { echo 'docker is required' >&2; exit 1; }
command -v openssl >/dev/null || { echo 'openssl is required' >&2; exit 1; }
command -v curl >/dev/null || { echo 'curl is required' >&2; exit 1; }
command -v od >/dev/null || { echo 'od is required' >&2; exit 1; }
command -v cmp >/dev/null || { echo 'cmp is required' >&2; exit 1; }
docker image inspect "$app_image" >/dev/null 2>&1 || {
	echo "application smoke image is missing: $app_image" >&2
	exit 1
}

work=$(mktemp -d /tmp/librepaper-deploy-install.XXXXXX)
case "$work" in /tmp/librepaper-deploy-install.*) ;; *) echo 'unsafe temporary path' >&2; exit 2;; esac
suffix="${work##*.}"
suffix="${suffix,,}"
fresh_project="lp-install-fresh-$suffix"
legacy_project="lp-install-legacy-$suffix"
fresh_env="$work/fresh.env"
legacy_env="$work/legacy.env"
fresh_config="$work/fresh.toml"
legacy_config="$work/legacy.toml"
fresh_override="$work/fresh-override.yaml"
legacy_override="$work/legacy-override.yaml"
legacy_fixture="$work/legacy-fixture.yaml"
legacy_data_volume="${legacy_project}_data"
legacy_postgres_volume="${legacy_project}_postgres"
fresh_owned=0
legacy_owned=0
seed_container_owned=0
seed_container="${legacy_project}-binary"
fresh_compose=()
legacy_compose=()
legacy_fixture_compose=()

cleanup() {
	local status=$?
	trap - EXIT
	if (( seed_container_owned )); then
		docker rm -f "$seed_container" >/dev/null 2>&1 || true
	fi
	if (( legacy_owned )); then
		if ((${#legacy_compose[@]})); then
			"${legacy_compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
		fi
		if ((${#legacy_fixture_compose[@]})); then
			"${legacy_fixture_compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
		fi
		docker volume rm "$legacy_postgres_volume" "$legacy_data_volume" >/dev/null 2>&1 || true
	fi
	if (( fresh_owned )); then
		if ((${#fresh_compose[@]})); then
			"${fresh_compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
		fi
		docker volume rm "${fresh_project}_postgres" "${fresh_project}_data" >/dev/null 2>&1 || true
	fi
	if [[ "${DEPLOY_INSTALL_KEEP:-0}" == 1 || $status != 0 ]]; then
		echo "deployment install smoke fixture preserved at $work" >&2
	else
		rm -rf -- "$work"
	fi
	exit "$status"
}
trap cleanup EXIT

assert_project_unused() {
	local project="$1" volume
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

random_edge_subnet() {
	local high low
	high=$(od -An -N1 -tu1 /dev/urandom | tr -d ' ')
	low=$(od -An -N1 -tu1 /dev/urandom | tr -d ' ')
	printf '10.%s.%s.0/24' "$((240 + high % 16))" "$((1 + low % 254))"
}

write_config() {
	local path="$1" host="$2"
	cat >"$path" <<TOML
[origins]
app = "http://$host"
docs = "http://docs.$host"

[storage]
directory = "/var/lib/librepaper"
database_url = { file = "/run/secrets/database_url" }

[server]
address = "0.0.0.0:8080"
migrate = false

[access]
publishers = ["any"]
commenters = ["any"]
TOML
	chmod 0444 "$path"
}

write_app_override() {
	local path="$1" subnet="$2"
	cat >"$path" <<YAML
services:
  migrate:
    image: $app_image
  librepaper:
    image: $app_image
    ports:
      - "127.0.0.1::8080"
networks:
  edge:
    ipam:
      config: !override
        - subnet: $subnet
YAML
}

compose_app_ready() {
	local name="$1" host="$2" compose_name="$3"
	local -n compose_ref="$compose_name"
	"${compose_ref[@]}" up -d postgres migrate librepaper || {
		"${compose_ref[@]}" logs --no-color postgres migrate librepaper >&2 || true
		return 1
	}
	local mapping port ready=0
	mapping=$("${compose_ref[@]}" port librepaper 8080)
	port="${mapping##*:}"
	[[ "$port" =~ ^[0-9]+$ ]] || { echo "could not determine published app port from $mapping" >&2; return 1; }
	for _ in {1..180}; do
		if curl -fsS -H "Host: $host" "http://127.0.0.1:$port/ready" -o /dev/null 2>/dev/null; then
			ready=1
			break
		fi
		sleep 1
	done
	[[ "$ready" == 1 ]] || {
		echo "app readiness endpoint did not respond at 127.0.0.1:$port" >&2
		"${compose_ref[@]}" logs --no-color librepaper migrate >&2 || true
		return 1
	}
	"${compose_ref[@]}" exec -T librepaper test -s /var/lib/librepaper/secrets/session.key
	echo "$name deployment is ready at 127.0.0.1:$port"
}

legacy_psql() {
	local role="$1" secret="$2"
	shift 2
	"${legacy_compose[@]}" exec -T postgres sh -c '
		PGPASSWORD=$(cat "$1")
		export PGPASSWORD
		shift
		exec "$@"
	' sh "/run/secrets/$secret" \
		psql -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -U "$role" -d librepaper "$@"
}

fresh_subnet=$(random_edge_subnet)
fresh_host="app-fresh-$suffix.example.test"
write_config "$fresh_config" "$fresh_host"
write_app_override "$fresh_override" "$fresh_subnet"
mkdir -m 700 "$work/fresh-secrets"
cat >"$fresh_env" <<ENV
COMPOSE_PROJECT_NAME=$fresh_project
LIBREPAPER_SECRETS_DIR=$work/fresh-secrets
LIBREPAPER_CONFIG_FILE=$fresh_config
ENV
assert_project_unused "$fresh_project"
"$deploy/setup" init --project "$fresh_project" \
	--secrets-dir "$work/fresh-secrets" --state-file "$work/fresh-state.json" \
	--env-file "$fresh_env" --config-file "$fresh_config"
fresh_db_secrets=(postgres_bootstrap_password database_owner_password database_app_password \
	database_backup_password database_metrics_password database_owner_url database_app_url \
	database_backup_url database_metrics_url database_metrics_uri database_metrics_user)
for secret in "${fresh_db_secrets[@]}"; do
	cp -- "$work/fresh-secrets/$secret" "$work/original-$secret"
	chmod 0600 "$work/original-$secret"
done
"$deploy/setup" init --project "$fresh_project" \
	--secrets-dir "$work/fresh-secrets" --state-file "$work/fresh-state.json" \
	--env-file "$fresh_env" --config-file "$fresh_config"
for secret in "${fresh_db_secrets[@]}"; do
	cmp "$work/original-$secret" "$work/fresh-secrets/$secret"
done
fresh_compose=(docker compose --project-directory "$deploy" --project-name "$fresh_project" \
	--env-file "$fresh_env" -f "$deploy/compose.yaml" -f "$fresh_override")
fresh_owned=1
compose_app_ready 'fresh' "$fresh_host" fresh_compose

# Build an old socket-only deployment with the same project-scoped volume names
# that deploy/setup upgrades. This service intentionally cannot join a network.
legacy_subnet=$(random_edge_subnet)
while [[ "$legacy_subnet" == "$fresh_subnet" ]]; do legacy_subnet=$(random_edge_subnet); done
legacy_host="app-legacy-$suffix.example.test"
write_config "$legacy_config" "$legacy_host"
write_app_override "$legacy_override" "$legacy_subnet"
legacy_secret_dir="$work/legacy-secrets"
mkdir -m 700 "$legacy_secret_dir"
cat >"$legacy_env" <<ENV
COMPOSE_PROJECT_NAME=$legacy_project
COMPOSE_FILE=compose.yaml
LIBREPAPER_SECRETS_DIR=$legacy_secret_dir
LIBREPAPER_CONFIG_FILE=$legacy_config
ENV
cat >"$legacy_fixture" <<YAML
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
      - $work/legacy-session.key:/session.key:ro
volumes:
  postgres:
    name: $legacy_postgres_volume
  data:
    name: $legacy_data_volume
YAML
chmod 0600 "$legacy_fixture"
printf '%s\n' "$(openssl rand -hex 32)" >"$work/legacy-session.key"
chmod 0600 "$work/legacy-session.key"
assert_project_unused "$legacy_project"
legacy_fixture_compose=(docker compose --project-directory "$deploy" --project-name "$legacy_project" -f "$legacy_fixture")
legacy_compose=(docker compose --project-directory "$deploy" --project-name "$legacy_project" \
	--env-file "$legacy_env" -f "$deploy/compose.yaml" -f "$legacy_override")
legacy_owned=1
"${legacy_fixture_compose[@]}" up -d --wait postgres
"${legacy_fixture_compose[@]}" run --rm seed-data

# Run the app image's migration executable against the old socket-trust server
# before the upgrade. This gives the fixture a real complete catalog/history.
"${legacy_fixture_compose[@]}" exec -T postgres mkdir -p /tmp/librepaper-upgrade-seed
if docker container inspect "$seed_container" >/dev/null 2>&1; then
	echo "refusing to reuse existing seed container $seed_container" >&2
	exit 2
fi
docker create --name "$seed_container" "$app_image" >/dev/null
seed_container_owned=1
docker cp "$seed_container:/usr/local/bin/librepaper" "$work/librepaper"
docker rm "$seed_container" >/dev/null
seed_container_owned=0
legacy_postgres_container=$("${legacy_fixture_compose[@]}" ps -q postgres)
docker cp "$work/librepaper" "$legacy_postgres_container:/tmp/librepaper"
cat >"$work/legacy-migrate.toml" <<'TOML'
[storage]
directory = "/tmp"
database_url = "postgresql://librepaper:legacy-disposable-password@127.0.0.1:5432/librepaper"
TOML
docker cp "$work/legacy-migrate.toml" "$legacy_postgres_container:/tmp/librepaper-upgrade-seed/config.toml"
docker exec "$legacy_postgres_container" chmod 0755 /tmp/librepaper
docker exec "$legacy_postgres_container" /tmp/librepaper admin migrate \
	--config /tmp/librepaper-upgrade-seed/config.toml
docker exec "$legacy_postgres_container" psql -X -v ON_ERROR_STOP=1 \
	-U librepaper -d librepaper -c \
	"INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status) VALUES ('7fc7d2d6-4522-4d1f-9cb2-c5d52bbdc3b8','registered','github','github:legacy-upgrade-smoke','legacy-smoke','Legacy Upgrade Smoke','active')" >/dev/null
legacy_volume_created_before=$(docker volume inspect -f '{{.CreatedAt}}' "$legacy_postgres_volume")

# upgrade must preserve the mounted database and data volumes while converting
# credentials/HBA, transferring schema ownership and disabling the old login.
COMPOSE_PROJECT_NAME="$legacy_project" COMPOSE_FILE=compose.yaml \
LIBREPAPER_SECRETS_DIR="$legacy_secret_dir" LIBREPAPER_CONFIG_FILE="$legacy_config" \
	"$deploy/setup" upgrade --yes --project "$legacy_project" \
	--secrets-dir "$legacy_secret_dir" --state-file "$work/legacy-state.json" \
	--env-file "$legacy_env" --config-file "$legacy_config"

compose_app_ready 'legacy-upgraded' "$legacy_host" legacy_compose
legacy_volume_created_after=$(docker volume inspect -f '{{.CreatedAt}}' "$legacy_postgres_volume")
[[ "$legacy_volume_created_before" == "$legacy_volume_created_after" ]] || {
	echo 'legacy PostgreSQL volume was replaced during upgrade' >&2
	exit 1
}

# The upgraded app-role connection can still see the seeded account, the
# legacy superuser can no longer log in, and existing objects now belong to the
# dedicated migration owner. Compare the private session key byte for byte.
app_account=$(legacy_psql librepaper_app database_app_password -Atc \
	"SELECT provider_subject FROM accounts WHERE id='7fc7d2d6-4522-4d1f-9cb2-c5d52bbdc3b8'")
[[ "$app_account" == github:legacy-upgrade-smoke ]] || {
	echo 'app role did not retain access to the legacy account' >&2
	exit 1
}
old_role_disabled=$(legacy_psql librepaper_bootstrap postgres_bootstrap_password -Atc \
	"SELECT NOT r.rolcanlogin AND NOT r.rolcreatedb AND NOT r.rolcreaterole AND (NOT r.rolsuper OR r.oid = 10) AND a.rolpassword IS NULL FROM pg_roles r JOIN pg_authid a USING (oid) WHERE r.rolname='librepaper'")
[[ "$old_role_disabled" == t ]] || { echo 'legacy superuser role was not disabled' >&2; exit 1; }
if legacy_psql librepaper_app database_app_password -c 'SET ROLE librepaper' >/dev/null 2>&1; then
	echo 'app role can still impersonate the legacy PostgreSQL role' >&2
	exit 1
fi
hba_scram=$(legacy_psql librepaper_bootstrap postgres_bootstrap_password -Atc \
	"SELECT EXISTS (SELECT 1 FROM pg_hba_file_rules WHERE type = 'host' AND auth_method = 'scram-sha-256' AND ('all' = ANY(database) OR 'librepaper' = ANY(database)))")
[[ "$hba_scram" == t ]] || { echo 'legacy PostgreSQL host authentication was not upgraded to SCRAM' >&2; exit 1; }
owner_check=$(legacy_psql librepaper_bootstrap postgres_bootstrap_password -Atc \
	"SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='public.accounts'::regclass")
[[ "$owner_check" == librepaper_owner ]] || { echo 'legacy schema ownership was not transferred to librepaper_owner' >&2; exit 1; }
legacy_app_container=$("${legacy_compose[@]}" ps -q librepaper)
docker cp "$legacy_app_container:/var/lib/librepaper/secrets/session.key" "$work/legacy-session-after.key"
cmp "$work/legacy-session.key" "$work/legacy-session-after.key"

echo 'fresh install and legacy PostgreSQL upgrade acceptance passed'
