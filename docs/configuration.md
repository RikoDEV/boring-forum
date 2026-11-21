# Configuration

Set environment variables; see `.env.example`. These enable optional features:

| Variable | Effect |
|---|---|
| `BF_REPLY_DOMAIN` | Topic emails get `Reply-To: reply+KEY@domain` |
| `BF_INBOUND_TOKEN` | Turns on `POST /api/v1/webhooks/inbound-mail` (32+ characters) |
| `BF_BOUNCE_TOKEN` | Turns on the bounce webhook (32+ characters) |
| `BF_PLUGIN_PUBLIC_KEYS` | Ed25519 keys trusted to sign plugins |
| `BF_TRUSTED_PROXIES` | Comma-separated proxy addresses whose `X-Forwarded-For` and `X-Forwarded-Proto` are believed. Set this behind a reverse proxy, or every visitor shares one address and `proxied` HTTPS redirects never happen |
| `BF_TICK_SECONDS` | Maintenance interval; default 600 |
| `BF_DELETE_GRACE_DAYS` | Days before a requested account deletion runs; default 14 |
| `BF_VAPID_PRIVATE`, `BF_VAPID_SUBJECT` | Web push; `boring-forum vapid` makes a key, and the subject is a `mailto:` or site address |
| `BF_SECRET_KEY` | **Encrypts TOTP and webhook secrets at rest.** `boring-forum secret-key` makes one. Also signs share-picture addresses. Without it those columns are stored in plain text and the server logs a warning at startup |
| `BF_ALLOW_NO_SECRET_KEY` | Set to `1` to start an https site without `BF_SECRET_KEY`. Without this the server refuses to start in that case (plain-http development is not affected) |
| `BF_SECRET_KEY_OLD` | Retired keys, comma separated, kept readable during a rotation |
| `BF_TLS_CERT`, `BF_TLS_KEY` | PEM certificate chain and key. Set both and the server terminates TLS itself with rustls (TLS 1.2 and 1.3 only) |
| `BF_HTTP_BIND` | With TLS enabled, an extra plain-HTTP listener (e.g. `0.0.0.0:80`) that only redirects to HTTPS |
| `BF_HTTPS_REDIRECT` | `off`, `proxied` or `always`. Defaults to `always` when terminating TLS, `proxied` for an `https://` base URL, `off` otherwise. `proxied` needs the proxy listed in `BF_TRUSTED_PROXIES`: `X-Forwarded-Proto` is ignored from any other peer |
| `BF_HSTS_PRELOAD` | Adds `preload` to the HSTS header. Opt-in: a preload submission is effectively permanent |
| `BF_MIGRATE_DATABASE_URL` | Runs migrations as a separate, schema-owning role; the server then connects with its own, lower-privilege role |
| `BF_EXPLORER_DATABASE_URL` | Enables the Data Explorer, running its queries on a connection that logs in as the restricted `bf_explorer` role. Unset keeps the Explorer off |
| `BF_STATEMENT_TIMEOUT_MS` | Per-connection statement timeout; default 15000 |
| `BF_AUDIT_SINK_STDOUT` | `true` writes every audit event to the `audit` tracing target for a log agent to collect |
| `BF_AUDIT_SINK_URL` | POSTs audit events as newline-delimited JSON to an external log system |
| `BF_AUDIT_SINK_SECRET`, `BF_AUDIT_SINK_TOKEN` | Signing key (32+ characters) and bearer token for that sink |

## Every environment variable

`scripts/check-env-docs.sh` (part of `scripts/check.sh` and CI) fails when the server reads a
variable that is not in this table and in `.env.example`. `boring-forum --check-config` loads the
environment as the server would and reports errors and doubtful settings without starting it.

| Variable | Default | Meaning |
|---|---|---|
| `BF_DATABASE_URL` | none (required) | Postgres connection string; must not be a superuser |
| `BF_MIGRATE_DATABASE_URL` | unset | Schema-owning role used only for migrations |
| `BF_DB_POOL_SIZE` | CPU count, at least 2 | Maximum database connections |
| `BF_STATEMENT_TIMEOUT_MS` | 15000 | Per-connection statement timeout |
| `BF_BIND` | `127.0.0.1:8080` | Listen address |
| `BF_BASE_URL` | `http://` + `BF_BIND` | Public origin for links, feeds and sitemaps. Required for `/readyz` |
| `BF_ADMIN_DIR` | `web/admin/build` | Directory with the built admin panel |
| `BF_ADMIN_TOKEN` | unset | Bearer token for the admin API, 32+ characters. A shorter value stops the server at startup |
| `BF_MICROCACHE_TTL_MS` | 2000 | Guest page cache lifetime |
| `BF_RATE_LIMIT_PER_MIN` | 600 | Requests per minute per client address |
| `BF_LOG_JSON` | false | One JSON object per log line |
| `BF_TRUSTED_PROXIES` | unset | Proxy addresses whose `X-Forwarded-*` headers are believed |
| `BF_GEO_HEADER`, `BF_ASN_HEADER` | unset | Trusted CDN headers with the visitor's country and network number |
| `BF_TLS_CERT`, `BF_TLS_KEY`, `BF_HTTP_BIND`, `BF_HTTPS_REDIRECT`, `BF_HSTS_PRELOAD` | see above | Built-in TLS, the plain-HTTP redirect listener and HSTS preload |
| `BF_DRAIN_SECS` | 5 | Seconds between failing `/readyz` and closing listeners on shutdown |
| `BF_TICK_SECONDS` | 600 | Maintenance interval |
| `BF_DELETE_GRACE_DAYS` | 14 | Days between a deletion request and the account being anonymized |
| `BF_SECRET_KEY`, `BF_SECRET_KEY_OLD` | unset | Encryption at rest for TOTP and webhook secrets, and retired keys during a rotation. Keep a copy of the current key apart from the database backups (see Backups) |
| `BF_UPLOAD_DIR` | `data/uploads` | Where uploaded files are stored on disk |
| `BF_SMTP_URL` | unset | Outgoing mail, e.g. `smtps://user:pass@smtp.example:465` |
| `BF_MAIL_FROM` | none | Sender mailbox, e.g. `Forum <noreply@example.com>`; required with `BF_SMTP_URL` |
| `BF_MAIL_DIR` | unset | Development: write each mail to a file here instead of sending it |
| `BF_REPLY_DOMAIN`, `BF_INBOUND_TOKEN`, `BF_BOUNCE_TOKEN` | unset | Reply by email, inbound mail webhook and bounce webhook |
| `BF_CAPTCHA_VERIFY_URL` | the provider's address | Verification endpoint override, used by tests |
| `BF_SSO_URL`, `BF_SSO_SECRET`, `BF_SSO_NAME` | unset | DiscourseConnect-style single sign-on |
| `BF_OIDC_NAME`, `BF_OIDC_CLIENT_ID`, `BF_OIDC_CLIENT_SECRET`, `BF_OIDC_AUTHORIZE_URL`, `BF_OIDC_TOKEN_URL`, `BF_OIDC_USERINFO_URL` | unset | A generic OpenID Connect provider; all but the name are required together |
| `BF_OAUTH_APPLE_ID`, `BF_OAUTH_APPLE_TEAM_ID`, `BF_OAUTH_APPLE_KEY_ID`, `BF_OAUTH_APPLE_KEY` | unset | Sign in with Apple |
| `BF_OAUTH_APPLE_BASE` | `https://appleid.apple.com` | Apple endpoint override, used by tests |
| `BF_AI_API_KEY`, `BF_AI_EMBEDDING_API_KEY` | unset | Keys for the AI provider and, when it is another one, the embedding model; they take precedence over the keys in the settings |
| `BF_MAILCHIMP_API_KEY` | unset | Mailchimp key (ends in `-dc`); turns the newsletter sync on |
| `BF_VAPID_PRIVATE`, `BF_VAPID_SUBJECT` | unset | Web push signing key and contact |
| `BF_METRICS_TOKEN` | unset | Bearer token (16+ characters) for `/metrics`; without one only a local scraper using a loopback `Host` is let in |
| `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME` | unset, `boring-forum` | OTLP trace export and the service name in it |
| `BF_CLUSTER` | unset | `1` on every node of a multi-node install: live updates, cache clears and announcements cross nodes through Postgres `LISTEN/NOTIFY` |
| `BF_IMAP_HOST`, `BF_IMAP_PORT`, `BF_IMAP_USER`, `BF_IMAP_PASSWORD`, `BF_IMAP_INTERVAL_SECS` | unset, 993, unset, unset, 60 | Poll a mailbox over IMAP (TLS) for reply-by-email instead of using the `/inbound` webhook |
| `BF_AI_PROVIDER` | unset | `mock` answers without a network (development and tests) |
| `BF_AI_ALLOW_PRIVATE` | unset | `1` lets the AI provider URL point at a private or loopback address (a local Ollama) |
| `BF_IMPORT_PLUGINS` | on | `0` skips plugin hooks while importing |
| `BF_REGISTER_LIMIT_PER_HOUR` | 10 | Registrations allowed per address per hour (raise it only for tests) |
| `BF_IMAGE_HASHLIST` | unset | File of known illegal image hashes |
| `BF_PLUGIN_PUBLIC_KEYS` | unset | Ed25519 keys trusted to sign plugins |
| `BF_EXPLORER_DATABASE_URL` | unset | Restricted connection for the Data Explorer |
| `BF_AUDIT_SINK_STDOUT`, `BF_AUDIT_SINK_URL`, `BF_AUDIT_SINK_SECRET`, `BF_AUDIT_SINK_TOKEN` | see above | Audit log forwarding |
| `BF_BACKUP_SCRIPT` | `scripts/backup.sh` | Script the `boring-forum backup` subcommand runs |
| `BF_TIME_SCALE_PERCENT` | 100 | Tests only: shortens background timers (1-100) |
| `BF_WEBHOOK_ALLOW_LOOPBACK` | unset | Tests only: `1` lets outgoing webhooks reach loopback addresses. Never set this in production |

## Security configuration

What an operator has to *do*:

### Encryption at rest

```sh
boring-forum secret-key >> .env     # prints BF_SECRET_KEY=…
```

Set it **before** the first member enables two-factor authentication. Keep it outside the
database and outside the backup — a backup encrypted with a key stored next to it is not
encrypted. Losing the key means every TOTP enrolment and webhook signing secret becomes
unreadable, and members have to re-enrol; it does not lock anyone out of their account.

Rotating:

```sh
# 1. Generate the new key and move the current one aside.
boring-forum secret-key                    # the new BF_SECRET_KEY
# .env: BF_SECRET_KEY=<new>   BF_SECRET_KEY_OLD=<previous>
# 2. Restart, so both keys are loaded.
# 3. Rewrite every row with the new key.
boring-forum rotate-secrets
# 4. When it reports nothing left on an old key, drop BF_SECRET_KEY_OLD and restart.
```

`rotate-secrets` is safe to re-run and only touches rows that are not already on the current key.

### Outside logins

The providers above are switched on by environment variables. How they behave is set in the admin
panel under Settings → Sign-up and sign-in:

| Setting | Default | What it does |
| --- | --- | --- |
| `auth_skip_create_confirm` | on | Off asks a new member to confirm their username on a page before the account is made. |
| `auth_immediately` | off | Sends guests straight to the outside login. Needs `login_required` and exactly one outside method; `/login?local=1` still shows the password form. |
| `auth_overrides_email`, `_username`, `_name`, `_avatar` | off | The provider's value replaces the local one on every login, and the member cannot change it. Applies to members who have an outside login. The email is only replaced when the provider has verified it; a username or email that another member already has is left alone. |
| `auth_require_interaction` | on | A visit to `/auth/<provider>` shows a button instead of starting the login, so another site cannot sign a visitor in by sending them there (login CSRF). The login page's buttons are forms and need no extra click. |

The provider's picture is fetched by the server (https only, up to 1 MB) and goes through the same
checks as an upload. Single sign-on reads `username`, `name` and `avatar_url` from the login site's payload.

### Database roles

By default the server connects as the owner of its own tables, which means a SQL injection that
got through could change the schema — and if that connection is a Postgres **superuser**, it
means code execution on the database host. Never run the server as a superuser: the server logs a
loud error at startup if `BF_DATABASE_URL` is one. `deploy/postgres/roles.sql` creates three roles
instead — `forum_migrate` (owns the schema, runs migrations), `forum_app` (the server: DML only)
and `forum_readonly` (ad-hoc queries, SELECT only, 5-second statement timeout):

```sh
psql "$SUPERUSER_URL" \
  -v app_password="'…'" -v migrate_password="'…'" -v readonly_password="'…'" \
  -f deploy/postgres/roles.sql
```

Then:

```sh
BF_DATABASE_URL=postgres://forum_app:…@db:5432/forum
BF_MIGRATE_DATABASE_URL=postgres://forum_migrate:…@db:5432/forum
```

Migrations run on their own short-lived connection and it is closed before the application pool
opens. `forum_app` has no `UPDATE` or `DELETE` on `audit_log`, which is append-only in the
database as well, so the log cannot be edited by the application at all. Audit
retention runs through two definer functions from the baseline schema.

With the repository's compose files, layer `deploy/compose.roles.yml` over `deploy/compose.yml` to do all of
this: `docker compose -f deploy/compose.yml -f deploy/compose.roles.yml up -d --wait`
creates the roles, migrates as `forum_migrate` and serves as `forum_app`. Its passwords default to
development values (`BF_APP_DB_PASSWORD`, `BF_MIGRATE_DB_PASSWORD`, `BF_READONLY_DB_PASSWORD` override them).
CI applies `roles.sql` and checks that `forum_app` cannot drop a table.

Run `deploy/postgres/roles.sql` again after upgrading an existing database, so tables the upgrade
added get their grants. A backup restore keeps the tables, so it needs no new grants. Installing a plugin that declares tables and the
`rebuild-search` task need schema rights: the server uses `BF_MIGRATE_DATABASE_URL` for them when set.

#### Data Explorer

The Data Explorer runs admin-written SQL. It is made safe by running each query on a connection
that logs in **as** the restricted `bf_explorer` role, which can read only the sanitized
`explorer.*` views. Because the confinement comes from the login role, a query cannot change role
back to the application's or use a SQL-executing built-in to read anything else. It is off unless
you configure that connection, so provision the role (after migrations) and point the server at it:

```sh
psql "$SUPERUSER_URL" -v explorer_password="'…'" -f deploy/postgres/explorer-role.sql
# then:
BF_EXPLORER_DATABASE_URL=postgres://bf_explorer:…@db:5432/forum
```

Never point `BF_EXPLORER_DATABASE_URL` at `forum_app` or a superuser; the server checks at startup
that the connection logs in as `bf_explorer` and is not a superuser, and disables the Explorer
otherwise.

### TLS

Behind a reverse proxy, terminate TLS there, set `BF_BASE_URL` to the `https://` address, and
make sure the proxy sends `X-Forwarded-Proto`. The server then redirects any request that arrives
as plain HTTP, and sends HSTS.

Without a proxy:

```sh
BF_TLS_CERT=/etc/ssl/forum/fullchain.pem
BF_TLS_KEY=/etc/ssl/forum/privkey.pem
BF_BIND=0.0.0.0:443
BF_HTTP_BIND=0.0.0.0:80      # redirects to HTTPS
```

Certificates are read at startup, so a renewal needs a restart.

### Shipping the audit log off the host

An audit log an attacker can also delete is worth little. Either:

```sh
BF_AUDIT_SINK_STDOUT=true   BF_LOG_JSON=true    # one JSON line per event, for a log agent
```

or push them:

```sh
BF_AUDIT_SINK_URL=https://logs.example/ingest
BF_AUDIT_SINK_SECRET=…        # events are signed like a webhook: t=<unix>,sha256=<hex>
BF_AUDIT_SINK_TOKEN=…         # optional bearer token
```

A cursor in the database records how far shipping has got, and it only advances once the sink has
accepted a batch, so a sink that is down makes events arrive late rather than not at all.

### Retention

The nightly housekeeping task (ten minutes after start, then every 24 hours; its result is in the
task log as `housekeeping`) does all of the following. Settings are in the admin, in days; 0
keeps the data.

| What | Window | Setting |
|---|---|---|
| IP address on audit events, posts, sessions and failed sign-ins | 180 | `ip_retention_days`. The event or post stays, only the address is cleared |
| Audit events | 0 (keep) | `audit_retention_days` |
| Post revisions (original and latest kept) | 0 (keep) | `edit_history_retention_days` |
| Deleted posts | 0 (keep) | `deleted_retention_days` |
| Task log, dismissed admin notices, link previews, profile visits | 90 | `operational_retention_days` |
| Notifications, read or not | 365 | `notification_retention_days` |
| Uploads no post, avatar or app uses | 7, fixed | |
| Failed sign-ins, search log, error log | 90, fixed | |
| Sent mail; webhook deliveries that finished | 30, fixed | |
| Mail that failed 5 or more times; webhooks still pending | 30, fixed | |
| Expired sessions (by date); expired API keys | at once; 30, fixed | |
| Expired sign-in codes; email tokens | 1 and 7, fixed | |
| Unsubscribe links | 365, fixed | |
| Idempotency keys; federation duplicate check | 24 hours; 14, fixed | |
| Sessions idle for `session_idle_minutes` | off | `session_idle_minutes`, swept every 10 minutes |
| Federation keys of deleted members | after the actor Delete has been delivered | |

`ap_objects` and `ap_dm_objects` are not purged: they tie remote content to posts that still
exist. Deleting an account (after the `BF_DELETE_GRACE_DAYS` grace) anonymizes it through the
registry in `crates/bf-db/src/accounts/lifecycle.rs`; each table there has its policy (delete,
null, scrub, or keep as a pseudonym), and orders keep amounts and invoice numbers but lose the
buyer's name, address and tax id. Federated followers get an actor `Delete`. The newsletter
flag is switched off, but the Mailchimp member itself is not removed yet: the sync pushes the
member's current address, which erasure has already replaced. Remove such members in Mailchimp.
