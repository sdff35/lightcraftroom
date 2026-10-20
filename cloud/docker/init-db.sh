#!/bin/sh
set -eu
LC_MIGRATOR_PASSWORD="$(cat /run/secrets/pg_migrator_password)"
LC_API_PASSWORD="$(cat /run/secrets/pg_api_password)"
LC_AUTH_PASSWORD="$(cat /run/secrets/pg_auth_password)"
export LC_MIGRATOR_PASSWORD LC_API_PASSWORD LC_AUTH_PASSWORD
psql --username "$POSTGRES_USER" --dbname postgres -v ON_ERROR_STOP=1 <<'SQL'
\getenv migrator_password LC_MIGRATOR_PASSWORD
\getenv api_password LC_API_PASSWORD
\getenv auth_password LC_AUTH_PASSWORD
CREATE ROLE lightcraft_migrator LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD :'migrator_password';
CREATE ROLE lightcraft_api LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD :'api_password';
CREATE ROLE lightcraft_auth LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD :'auth_password';
CREATE DATABASE lightcraft OWNER lightcraft_migrator;
CREATE DATABASE lightcraft_auth OWNER lightcraft_auth;
REVOKE ALL ON DATABASE lightcraft FROM PUBLIC;
REVOKE ALL ON DATABASE lightcraft_auth FROM PUBLIC;
GRANT CONNECT ON DATABASE lightcraft TO lightcraft_api;
CREATE TABLE public.lightcraft_bootstrap_complete(version integer NOT NULL);
INSERT INTO public.lightcraft_bootstrap_complete VALUES (1);
SQL
