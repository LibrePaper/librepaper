#!/bin/sh
# Create or rotate the aggregate-statistics role without placing its password
# in the psql command line. This is also mounted into PostgreSQL's first-run
# initialization directory and can be rerun against existing clusters.
set -eu

psql --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" --set=ON_ERROR_STOP=1 <<'SQL'
\getenv metrics_password POSTGRES_EXPORTER_PASSWORD
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L', 'librepaper_metrics', :'metrics_password')
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'librepaper_metrics')
\gexec
SELECT format('ALTER ROLE %I LOGIN PASSWORD %L', 'librepaper_metrics', :'metrics_password')
\gexec
GRANT pg_monitor TO librepaper_metrics;
SQL
