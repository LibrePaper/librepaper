\getenv owner_password LIBREPAPER_OWNER_PASSWORD
\getenv app_password LIBREPAPER_APP_PASSWORD
\getenv backup_password LIBREPAPER_BACKUP_PASSWORD
\getenv metrics_password LIBREPAPER_METRICS_PASSWORD

CREATE ROLE librepaper_owner LOGIN PASSWORD :'owner_password'
    NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT;
CREATE ROLE librepaper_app LOGIN PASSWORD :'app_password'
    NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT;
CREATE ROLE librepaper_backup LOGIN PASSWORD :'backup_password'
    NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT;
CREATE ROLE librepaper_metrics LOGIN PASSWORD :'metrics_password'
    NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION INHERIT;

ALTER DATABASE librepaper OWNER TO librepaper_owner;
REVOKE CONNECT ON DATABASE librepaper FROM PUBLIC;
GRANT CONNECT ON DATABASE librepaper TO librepaper_app, librepaper_backup, librepaper_metrics;

\connect librepaper

ALTER SCHEMA public OWNER TO librepaper_owner;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO librepaper_app, librepaper_backup;

-- The app role may do row and sequence DML, and inspect migration history,
-- while all schema changes remain exclusive to the one-shot owner role.
ALTER DEFAULT PRIVILEGES FOR ROLE librepaper_owner IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO librepaper_app;
ALTER DEFAULT PRIVILEGES FOR ROLE librepaper_owner IN SCHEMA public
    GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO librepaper_app;

-- Backups use pg_dump and must read every application table and sequence.
ALTER DEFAULT PRIVILEGES FOR ROLE librepaper_owner IN SCHEMA public
    GRANT SELECT ON TABLES TO librepaper_backup;
ALTER DEFAULT PRIVILEGES FOR ROLE librepaper_owner IN SCHEMA public
    GRANT SELECT ON SEQUENCES TO librepaper_backup;

GRANT pg_monitor TO librepaper_metrics;
