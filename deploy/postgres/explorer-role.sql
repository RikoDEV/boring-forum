-- The Data Explorer's login role.
--
-- The Explorer runs untrusted admin SQL. It is made safe by running it on a connection that logs
-- in AS this role, which can read only the sanitized `explorer.*` views (no password hashes, 2FA
-- secrets, tokens, IPs or private messages) and owns nothing. Because confinement comes from the
-- login role, a submitted query cannot `RESET ROLE` into a privileged role, nor use a built-in
-- that executes SQL to read outside the views.
--
-- Run migrations first (the baseline migration creates the `explorer` schema, its views and the NOLOGIN
-- bf_explorer role; 0094 strips the dangerous built-ins). Then, as a superuser:
--
--   psql "$SUPERUSER_URL" -v explorer_password="'…'" -f deploy/postgres/explorer-role.sql
--
-- and point the server at it:
--
--   BF_EXPLORER_DATABASE_URL=postgres://bf_explorer:…@db:5432/forum
--
-- Leaving BF_EXPLORER_DATABASE_URL unset keeps the Data Explorer switched off.

\set ON_ERROR_STOP on

-- Migration 0077 already created the role (NOLOGIN) and granted it the explorer views. If that
-- migration could not create it (the database did not allow CREATE ROLE), create it now.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bf_explorer') THEN
        CREATE ROLE bf_explorer NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
        GRANT USAGE ON SCHEMA explorer TO bf_explorer;
        GRANT SELECT ON ALL TABLES IN SCHEMA explorer TO bf_explorer;
    END IF;
END
$$;

-- Give it a password and let it log in. The password is substituted by psql outside any
-- dollar-quoted block, exactly as deploy/postgres/roles.sql does for the other roles.
ALTER ROLE bf_explorer LOGIN PASSWORD :explorer_password;
ALTER ROLE bf_explorer NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;

-- Belt and braces alongside the per-transaction settings the server applies.
ALTER ROLE bf_explorer SET statement_timeout = '10s';
ALTER ROLE bf_explorer SET idle_in_transaction_session_timeout = '15s';
ALTER ROLE bf_explorer SET default_transaction_read_only = on;

-- It owns no tables and must not reach the catalog of credentials.
REVOKE ALL ON pg_authid FROM bf_explorer;
