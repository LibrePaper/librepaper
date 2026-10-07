-- Socket trust, no passwords. Superuser postgres used by nobody.
CREATE ROLE librepaper LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE;
CREATE DATABASE librepaper OWNER librepaper;
