#!/bin/sh
# Optional Linux client wrapper for the disposable /tmp-based drill.
# Invoke through symlinks named psql, pg_dump, or pg_restore.
set -eu
tool=${0##*/}
case "$tool" in
  psql|pg_dump|pg_restore) ;;
  *) echo 'invoke through a psql, pg_dump, or pg_restore symlink' >&2; exit 2 ;;
esac
exec docker run --rm -i --network host -v /tmp:/tmp \
  -e PGDATABASE -e PGPASSWORD postgres:17-alpine "$tool" "$@"
