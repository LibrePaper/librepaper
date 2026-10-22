#!/bin/sh
# resticprofile runs this after a backup. When the export left stored files
# out, it fails the run, so the failure alert fires although the snapshot was
# saved. The next backup that finds every file clears it.
set -eu
count=$(cat /run/librepaper-backup/missing-objects 2>/dev/null || echo 0)
if [ "${count:-0}" -gt 0 ]; then
	echo "librepaper-backup: $count stored files were missing from object storage and are not in this snapshot; see the WARNING lines above" >&2
	exit 1
fi
