#!/bin/sh
set -eu

CONFIG=${LIBREPAPER_CONFIG_FILE:-${LIBREPAPER_BACKUP_CONFIG:-/etc/librepaper/config.toml}}
PROFILE=${LIBREPAPER_BACKUP_PROFILE:-/etc/resticprofile/profiles.toml}
CRONTAB=${LIBREPAPER_BACKUP_CRONTAB:-/run/librepaper-backup/crontab}
validate_only=${LIBREPAPER_BACKUP_VALIDATE_ONLY:-0}

# Parse all TOML before deciding whether the optional backup section exists.
# The helper prints only a state word and never echoes configuration values.
if ! state=$(python3 - "$CONFIG" <<'PY'
import sys
import tomllib

try:
    with open(sys.argv[1], "rb") as source:
        config = tomllib.load(source)
except (OSError, tomllib.TOMLDecodeError):
    print("invalid")
    raise SystemExit(2)

if "backup" in config:
    print("legacy")
elif "resticprofile" not in config:
    print("disabled")
elif not isinstance(config["resticprofile"], dict):
    print("wrong-type")
else:
    print("enabled")
PY
); then
	# A malformed config cannot establish whether backups were enabled. Missing
	# metrics are independently alerted by the monitoring rules.
	[ "$validate_only" = 1 ] || librepaper-backup-status config-error || true
	echo "backup startup: invalid configuration TOML" >&2
	exit 1
fi

case "$state" in
disabled)
		[ "$validate_only" = 1 ] && exit 0
		librepaper-backup-status disabled
		exec sleep infinity
		;;
enabled)
		# Persist the enabled sentinel before merged-profile validation so invalid
		# settings remain visible to monitoring after the process exits.
		[ "$validate_only" = 1 ] || librepaper-backup-status enabled
		;;
	legacy)
		[ "$validate_only" = 1 ] || librepaper-backup-status config-error || true
		echo "backup startup: legacy [backup] configuration is unsupported" >&2
		exit 1
		;;
	wrong-type)
		[ "$validate_only" = 1 ] || librepaper-backup-status config-error || true
		echo "backup startup: [resticprofile] must be a TOML table" >&2
		exit 1
		;;
	*)
		echo "backup startup: invalid configuration" >&2
		exit 1
		;;
esac

# `show` validates the merged profile; discard its output because it can
# contain repository credentials supplied by the operator.
if ! resticprofile -c "$PROFILE" -n resticprofile show >/dev/null 2>&1; then
	[ "$validate_only" = 1 ] || librepaper-backup-status config-error || true
	echo "backup startup: merged profile validation failed" >&2
	exit 1
fi
if ! resticprofile -c "$PROFILE" -n resticprofile schedule >/dev/null 2>&1; then
	[ "$validate_only" = 1 ] || librepaper-backup-status config-error || true
	echo "backup startup: schedule installation failed" >&2
	exit 1
fi
[ -s "$CRONTAB" ] || { [ "$validate_only" = 1 ] || librepaper-backup-status config-error || true; echo "backup startup: schedule file is empty" >&2; exit 1; }
[ "$validate_only" = 1 ] && exit 0
exec supercronic "$CRONTAB"
