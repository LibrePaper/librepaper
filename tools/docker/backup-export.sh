#!/bin/sh
# Writes the whole backup as one tar archive on stdout for
# `restic backup --stdin-from-command`. The recovery point keeps the path the
# sidecar used to stage it, so old and new snapshots unpack to the same tree.
# It also counts the stored files the export had to leave out, for
# librepaper-backup-missing-check.
set -eu
state=/run/librepaper-backup
log=$state/export.log
status=0
librepaper admin backup \
	--config /etc/librepaper/librepaper.toml \
	--prefix var/backups/librepaper/current \
	--include /etc/librepaper/librepaper.toml \
	--include /etc/resticprofile/resticprofile.toml \
	--include /var/lib/librepaper/secrets \
	- 2>"$log" || status=$?
cat "$log" >&2
grep -c '^WARNING: backup object ' "$log" >"$state/missing-objects" || true
rm -f "$log"
exit "$status"
