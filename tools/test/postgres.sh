#!/usr/bin/env bash
# Shared lifecycle for disposable local PostgreSQL containers. Source this
# file from a Bash script, then call postgres_start_disposable and
# postgres_stop_disposable. Persistent development databases stay in tools/dev/db.

POSTGRES_IMAGE="postgres:17.11-alpine"

postgres_start_disposable() {
  local name="$1" password="$2" database="${3:-postgres}" attempts="${4:-60}"
  command -v docker >/dev/null || { echo "error: docker is required" >&2; return 1; }
  docker run -d --rm --name "$name" \
    -e POSTGRES_PASSWORD="$password" -e POSTGRES_DB="$database" \
    -p 127.0.0.1::5432 "$POSTGRES_IMAGE" >/dev/null || return
  POSTGRES_CONTAINER="$name"
  local mapping
  mapping=$(docker port "$name" 5432/tcp 2>/dev/null | head -1) || {
    echo "error: Docker did not report the PostgreSQL port mapping" >&2
    postgres_stop_disposable
    return 1
  }
  if [[ ! "$mapping" =~ ^127\.0\.0\.1:([0-9]+)$ ]]; then
    echo "error: Docker did not publish PostgreSQL on a loopback port: $mapping" >&2
    postgres_stop_disposable
    return 1
  fi
  POSTGRES_PORT="${BASH_REMATCH[1]}"
  local ready=0 i
  for (( i=0; i<attempts; i++ )); do
    if docker exec "$name" pg_isready -q -U postgres -h 127.0.0.1 &&
       (echo >"/dev/tcp/127.0.0.1/$POSTGRES_PORT") >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 1
  done
  if [[ "$ready" != 1 ]]; then
    echo "error: PostgreSQL did not become ready on 127.0.0.1:$POSTGRES_PORT" >&2
    postgres_stop_disposable
    return 1
  fi
}

postgres_stop_disposable() {
  if [[ -n "${POSTGRES_CONTAINER:-}" ]]; then
    docker rm -f "$POSTGRES_CONTAINER" >/dev/null 2>&1 || true
    unset POSTGRES_CONTAINER POSTGRES_PORT
  fi
}
