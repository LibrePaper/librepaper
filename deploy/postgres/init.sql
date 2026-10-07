-- Socket trust, no passwords. Superuser postgres used by nobody.
CREATE ROLE librepaper LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE;
CREATE DATABASE librepaper OWNER librepaper;
CREATE ROLE librepaper_metrics LOGIN NOSUPERUSER;
GRANT pg_monitor TO librepaper_metrics;
