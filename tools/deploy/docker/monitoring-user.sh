#!/bin/sh
# Create or rotate the aggregate-statistics role. The exporter connects over the
# local socket, which the cluster trusts, so the role carries no password.
# Rerunnable.
set -eu

psql --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" --set=ON_ERROR_STOP=1 <<'SQL'
SELECT 'CREATE ROLE librepaper_metrics LOGIN' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'librepaper_metrics')
\gexec
ALTER ROLE librepaper_metrics LOGIN PASSWORD NULL;
GRANT pg_monitor TO librepaper_metrics;
SQL
