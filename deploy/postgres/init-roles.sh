#!/bin/sh
set -eu

LIBREPAPER_OWNER_PASSWORD=$(cat /run/secrets/database_owner_password)
LIBREPAPER_APP_PASSWORD=$(cat /run/secrets/database_app_password)
LIBREPAPER_BACKUP_PASSWORD=$(cat /run/secrets/database_backup_password)
LIBREPAPER_METRICS_PASSWORD=$(cat /run/secrets/database_metrics_password)
export LIBREPAPER_OWNER_PASSWORD LIBREPAPER_APP_PASSWORD
export LIBREPAPER_BACKUP_PASSWORD LIBREPAPER_METRICS_PASSWORD
[ -n "$LIBREPAPER_OWNER_PASSWORD" ] || { echo 'postgres init: empty owner password' >&2; exit 1; }
[ -n "$LIBREPAPER_APP_PASSWORD" ] || { echo 'postgres init: empty app password' >&2; exit 1; }
[ -n "$LIBREPAPER_BACKUP_PASSWORD" ] || { echo 'postgres init: empty backup password' >&2; exit 1; }
[ -n "$LIBREPAPER_METRICS_PASSWORD" ] || { echo 'postgres init: empty metrics password' >&2; exit 1; }

psql --set=ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres \
	--file=/usr/local/share/librepaper/roles.sql
