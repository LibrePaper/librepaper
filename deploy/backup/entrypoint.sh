#!/bin/sh
set -eu

CONFIG=${LIBREPAPER_BACKUP_CONFIG:-/etc/resticprofile/resticprofile.toml}
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
	echo "backup startup: invalid configuration TOML" >&2
	exit 1
fi

case "$state" in
disabled)
		[ "$validate_only" = 1 ] && exit 0
		echo "backup startup: backups are disabled; configure [resticprofile] in resticprofile.toml to enable scheduled backups"
		exec sleep infinity
	;;
enabled)
	;;
legacy)
	echo "backup startup: legacy [backup] configuration is unsupported; configure resticprofile.toml" >&2
	exit 1
	;;
wrong-type)
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
	echo "backup startup: merged profile validation failed" >&2
	exit 1
fi
if ! resticprofile -c "$PROFILE" -n resticprofile schedule >/dev/null 2>&1; then
	echo "backup startup: schedule installation failed" >&2
	exit 1
fi
[ -s "$CRONTAB" ] || { echo "backup startup: schedule file is empty" >&2; exit 1; }
[ "$validate_only" = 1 ] && exit 0
exec /usr/bin/supercronic "$CRONTAB"
