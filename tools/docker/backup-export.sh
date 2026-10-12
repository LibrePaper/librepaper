#!/bin/sh
# Writes the whole backup as one tar archive on stdout for
# `restic backup --stdin-from-command`. The recovery point keeps the path the
# sidecar used to stage it, so old and new snapshots unpack to the same tree.
set -eu
exec librepaper admin backup \
	--config /etc/librepaper/librepaper.toml \
	--prefix var/backups/librepaper/current \
	--include /etc/librepaper/librepaper.toml \
	--include /etc/resticprofile/resticprofile.toml \
	--include /var/lib/librepaper/secrets \
	-
