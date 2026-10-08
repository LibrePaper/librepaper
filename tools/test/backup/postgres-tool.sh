#!/usr/bin/env bash
# PostgreSQL clients in Docker, for a machine without matching ones. Invoke
# through symlinks named psql, pg_dump or pg_restore.
set -eu
. "$(dirname "$(readlink -f "$0")")/../postgres.sh"
tool=${0##*/}
case "$tool" in
  psql|pg_dump|pg_restore) ;;
  *) echo 'invoke through a psql, pg_dump, or pg_restore symlink' >&2; exit 2 ;;
esac
exec docker run --rm -i --network host -v /tmp:/tmp \
  -e PGDATABASE -e PGPASSWORD "$POSTGRES_IMAGE" "$tool" "$@"
