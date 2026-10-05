#!/bin/sh
set -eu

password_file=${GF_SECURITY_ADMIN_PASSWORD__FILE:-/run/secrets/grafana_admin_password}
run_script=${LIBREPAPER_GRAFANA_RUN_SCRIPT:-/run.sh}
if [ ! -r "$password_file" ] || ! grep -q '[^[:space:]]' "$password_file"; then
	echo 'grafana startup: admin password secret is missing or empty' >&2
	exit 1
fi

exec "$run_script" "$@"
