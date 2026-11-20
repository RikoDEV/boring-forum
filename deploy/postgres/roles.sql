-- Least-privilege database roles.
--
-- The forum must not run as the owner of its own tables: a SQL injection that gets through would
-- otherwise be able to DROP them or read `pg_authid`. Three roles, each with the smallest set of
-- rights that lets it do its job:
--
--   forum_migrate   owns the schema and runs migrations (DDL). Used by `boring-forum migrate`
--                   and by a deploy step, never by the running server.
--   forum_app       the server. DML on the existing tables only: no CREATE, no DROP, no
--                   ownership, and therefore no way to change the schema or the triggers that
--                   make audit_log append-only.
--   forum_readonly  ad-hoc queries and reporting. SELECT only, with a short statement timeout so
--                   an expensive query cannot affect the forum.
--   bf_explorer     the Data Explorer's own login role. It can read only the
--                   sanitized `explorer.*` views and nothing else, so untrusted admin SQL runs
--                   confined even if it changes role or calls a SQL-executing built-in.
--
-- Run migrations first (so the `explorer` schema and views exist), then, as a superuser against
-- the forum database:
--
--   psql "$SUPERUSER_URL" \
--     -v app_password="'…'" -v migrate_password="'…'" -v readonly_password="'…'" \
--     -v explorer_password="'…'" \
--     -f deploy/postgres/roles.sql
--
-- Then point each connection at its role:
--
--   BF_DATABASE_URL=postgres://forum_app:…@db:5432/forum
--   BF_MIGRATE_DATABASE_URL=postgres://forum_migrate:…@db:5432/forum
--   BF_EXPLORER_DATABASE_URL=postgres://bf_explorer:…@db:5432/forum
--     (enables the Data Explorer; provision this role with deploy/postgres/explorer-role.sql)
--
-- Re-running this file is safe: every statement is idempotent, and it grants on whatever tables
-- exist now plus, through the default privileges below, on any a later migration adds.

\set ON_ERROR_STOP on

-- Roles. `NOSUPERUSER NOCREATEDB NOCREATEROLE` is the default, and spelled out here so a reader
-- does not have to know that.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'forum_migrate') THEN
        CREATE ROLE forum_migrate LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'forum_app') THEN
        CREATE ROLE forum_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'forum_readonly') THEN
        CREATE ROLE forum_readonly LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    END IF;
END
$$;

ALTER ROLE forum_migrate  PASSWORD :migrate_password;
ALTER ROLE forum_app      PASSWORD :app_password;
ALTER ROLE forum_readonly PASSWORD :readonly_password;

-- Per-role limits. The app's is set per connection as well (see `bf_db::connect`); this covers
-- any other client that logs in as the role.
ALTER ROLE forum_app      SET statement_timeout = '15s';
ALTER ROLE forum_app      SET idle_in_transaction_session_timeout = '30s';
ALTER ROLE forum_readonly SET statement_timeout = '5s';
ALTER ROLE forum_readonly SET idle_in_transaction_session_timeout = '10s';
ALTER ROLE forum_readonly SET default_transaction_read_only = on;
-- Migrations take locks and rewrite tables, so they get room; `lock_timeout` keeps a blocked
-- migration from queueing behind a long read and stalling the whole forum.
ALTER ROLE forum_migrate  SET statement_timeout = '0';
ALTER ROLE forum_migrate  SET lock_timeout = '10s';

-- The migration role owns the schema; the other two only use it.
ALTER SCHEMA public OWNER TO forum_migrate;
-- Trusted extensions such as pg_trgm (migration 0002) need CREATE on the database.
DO $$ BEGIN EXECUTE format('GRANT CREATE ON DATABASE %I TO forum_migrate', current_database()); END $$;
-- An existing database: hand over what the old role owns, or migrations cannot change it.
DO $$
DECLARE t record;
BEGIN
    FOR t IN SELECT tablename FROM pg_tables WHERE schemaname = 'public' LOOP
        EXECUTE format('ALTER TABLE public.%I OWNER TO forum_migrate', t.tablename);
    END LOOP;
END;
$$;
GRANT USAGE ON SCHEMA public TO forum_app, forum_readonly;
-- Nobody creates objects in `public` except the migration role.
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT CREATE ON SCHEMA public TO forum_migrate;

-- Rights on what exists today.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO forum_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO forum_app;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO forum_readonly;

-- …and on what later migrations add, since they run as forum_migrate.
ALTER DEFAULT PRIVILEGES FOR ROLE forum_migrate IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO forum_app;
ALTER DEFAULT PRIVILEGES FOR ROLE forum_migrate IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO forum_app;
ALTER DEFAULT PRIVILEGES FOR ROLE forum_migrate IN SCHEMA public
    GRANT SELECT ON TABLES TO forum_readonly;

-- The audit log is append-only for the application. The triggers from migration 0057
-- refuse UPDATE and DELETE whatever the role, and this takes the rights away as well, so the
-- two controls are independent: dropping a trigger needs ownership, which forum_app lacks.
-- On a fresh database the table does not exist yet; migration 0065 repeats this.
DO $$
BEGIN
    IF to_regclass('public.audit_log') IS NOT NULL THEN
        REVOKE UPDATE, DELETE, TRUNCATE ON audit_log FROM forum_app;
    END IF;
END;
$$;

-- Nothing should read the password hashes or 2FA secrets through the read-only role.
DO $$
DECLARE t text;
BEGIN
    IF to_regclass('public.users') IS NOT NULL THEN
        REVOKE SELECT ON users FROM forum_readonly;
        GRANT SELECT (id, username, email_verified_at, joined_at) ON users TO forum_readonly;
    END IF;
    IF to_regclass('public.webhooks') IS NOT NULL THEN
        REVOKE SELECT ON webhooks FROM forum_readonly;
        GRANT SELECT (id, url, events, active, created_at) ON webhooks TO forum_readonly;
    END IF;
    -- Credentials stay out of reach of ad-hoc queries.
    FOREACH t IN ARRAY ARRAY['sessions', 'backup_codes', 'api_keys', 'user_identities', 'passkeys',
                             'email_tokens', 'pending_logins', 'sso_nonces'] LOOP
        IF to_regclass('public.' || t) IS NOT NULL THEN
            EXECUTE format('REVOKE SELECT ON public.%I FROM forum_readonly', t);
        END IF;
    END LOOP;
END;
$$;

-- Neither application role may read the catalog of roles and passwords.
REVOKE ALL ON pg_authid FROM forum_app, forum_readonly;

-- The Data Explorer's login role (bf_explorer) is provisioned separately, only on sites that
-- want the Explorer: see deploy/postgres/explorer-role.sql.
