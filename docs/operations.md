# Operating a forum

## Monitoring

| Endpoint | What it says |
|---|---|
| `/healthz`, `/livez` | the process answers (200, `{"status":"ok"}`). No database or other I/O, so a database outage does not restart every instance. Use for liveness probes |
| `/readyz` | 200 only when the database answers within 2 s, every embedded migration is applied, the required configuration (`BF_BASE_URL`) is present and the process is not draining; otherwise 503. The JSON body says which part failed: `{"status","db","migrations","config","draining"}`. Use for load balancer and orchestrator readiness |
| `/metrics` | Prometheus text format; `Authorization: Bearer $BF_METRICS_TOKEN` (16 characters or more), or, when no token is set, loopback peers that address the server by a loopback `Host` (`localhost`, `127.0.0.1`, `[::1]`) and send no `X-Forwarded-For`, `Forwarded` or `X-Real-IP`, so a same-host reverse proxy passing the public `Host` does not expose it. Anyone else gets 404 |

The probes and `/metrics` are exempt from the HTTPS redirect and from the per-IP rate limit, so a
kubelet, load balancer or scraper can reach the instance directly and often. The container image
has no curl: its `HEALTHCHECK` (and the compose `healthcheck`) runs `boring-forum healthcheck`,
which asks the instance's own `/readyz` over loopback and exits 0 on a 200.

### Metrics

Routes are labelled by their template (`/topic/{id}`), never the raw path, so a topic does not
get its own series. `status` is the full code (`200`, `404`); aggregate with `status=~"5.."`.

| Series | Type | Labels |
|---|---|---|
| `bf_http_requests_total` | counter | `method`, `route`, `status` |
| `bf_http_request_duration_seconds` | histogram | `method`, `route` |
| `bf_db_pool_connections`, `bf_db_pool_idle` | gauge | set at scrape time |
| `bf_queue_depth` | gauge | `queue` (mail, webhooks, push, plugin jobs, federation deliveries) |
| `bf_realtime_subscribers` | gauge | live WebSocket/SSE subscribers |
| `bf_rate_limited_total` | counter | requests refused by the rate limiter |
| `bf_csp_violations_total` | counter | `directive` |
| `bf_known_image_matches_total` | counter | |

Every request has an `X-Request-Id` (a UUID, or the caller's value), returned in the response
header and carried as `id` on the request span, so it is on every log line written while handling
it (`BF_LOG_JSON=true` for structured logs).

### Alert suggestions

- `/readyz` failing for 1 minute: page. A failing `/healthz` means the process is wedged: restart.
- `histogram_quantile(0.95, sum by (le, route) (rate(bf_http_request_duration_seconds_bucket[5m])))`
  above 50 ms for pages: warn.
- `sum(rate(bf_http_requests_total{status=~"5.."}[5m])) / sum(rate(bf_http_requests_total[5m])) > 0.01`: page.
- `bf_queue_depth{queue}` growing for 15 minutes: mail, webhooks or deliveries are stuck.
- `bf_db_pool_idle == 0` for 5 minutes with `bf_db_pool_connections` at the configured pool size: the pool is too small or queries are stuck.
- `rate(bf_rate_limited_total[5m])` rising sharply: abuse, or a proxy missing from `BF_TRUSTED_PROXIES`.
- `bf_csp_violations_total`: a new directive after a deploy usually means a broken page.
- `bf_known_image_matches_total`: page someone.

Traces go to an OpenTelemetry collector when `OTEL_EXPORTER_OTLP_ENDPOINT` is set (OTLP/HTTP).
An incoming W3C `traceparent` makes the request part of the caller's trace, and federation and
link-preview requests carry the trace onward. OTLP export is opt-in: without
`OTEL_EXPORTER_OTLP_ENDPOINT` no exporter is built and nothing leaves the host. `OTEL_SERVICE_NAME`
and the other `OTEL_*` variables are read by the SDK. Request spans carry method, URI (credentials
in query strings redacted) and request id. SQL statements are logged by sqlx at debug level:
`RUST_LOG=info,sqlx=debug` turns them on for a debugging session (the default is `info,sqlx=warn`).

### Shutdown

On SIGTERM or Ctrl-C the server marks itself draining (`/readyz` answers 503, so the load balancer
stops sending traffic), waits `BF_DRAIN_SECS` (default 5), then stops accepting connections and
lets requests in flight finish. Give the orchestrator a grace period longer than that plus your
slowest request (compose uses `stop_grace_period: 30s`).

## Backup and restore

A backup is one file, `boring-forum-<date>-<time>-v<schema>.tar.gz`, holding the whole database and,
if you ask for them, the uploaded files. Manage them in the admin panel under **Backups**: back up
now, download, restore and delete, and turn on automatic backups.

| Setting (Admin, Backups) | Default | |
| --- | --- | --- |
| `backup_enabled` | off | Take a backup on a schedule |
| `backup_interval_hours` | 24 | Hours between automatic backups |
| `backup_keep` | 7 | Backups to keep; the oldest are deleted after each new one |
| `backup_uploads` | on | Put the uploaded files in automatic backups |

Archives are written to `BF_BACKUP_DIR` (`data/backups`; `/data/backups`, a volume, in the container
image). The server checks every ten minutes (`BF_TICK_SECONDS`) whether a backup is due.

**Treat every archive as a secret.** It holds every password hash, email address and private
message. Files are written readable by the server's user only. **Download** does not hand the file
to a session: it emails a link to the address on your account. The link works once, within 15
minutes, and only while you are signed in as that administrator, so a stolen session or a mail
scanner cannot fetch an archive. Archives are not encrypted: keep the folder on
an encrypted disk, and encrypt the copy you move off the machine (for example with
`age -r <public key> file.tar.gz`). Copy backups off the machine regularly; a backup next to the
database does not survive losing the host.

### What is in a backup

- `manifest.json`: the version, the schema version, when it was made and what it holds.
- `db/<table>.copy`: every table, read from a single consistent snapshot, so a backup taken
  while the forum is busy is still self-consistent.
- `uploads/…`: the uploaded files, when included.

The schema is not in the archive: it comes from the server's own migrations. A backup restores into
a database the server has already migrated, so restoring is always done with a server at least as
new as the one that made the backup. A backup from an older version loads into a newer one as long
as the newer schema only added things.

### Restoring

In the admin panel, press **Restore** on a backup and type `restore`. The site goes offline, the
backup replaces every table in one transaction (either all of it loads or nothing changes), the
uploaded files are put back, the caches are reloaded and the site comes back as the backup had it.
Everyone, you included, has to sign in again, because sessions are part of what was restored.

The restore needs a database role that owns the tables. With `BF_MIGRATE_DATABASE_URL` set it uses
that role, so the application's own role can stay unable to change the schema.

From the command line, on a server that is not running:

```sh
boring-forum backup [dir]                  # write a backup into dir (default BF_BACKUP_DIR)
boring-forum restore <file.tar.gz> [--yes] # replace the database and uploads with a backup
```

To move a site to a new server, install the same or a newer version there, start it once so the
database is migrated, copy the archive into its backup folder and restore it from the panel (or run
`boring-forum restore`). Set the same `BF_SECRET_KEY` first.

### Rolling back an upgrade

Take a backup before you upgrade (Backups, **Back up now**). If the new version is bad, deploy the
previous version of the server against an empty database, start it once, then restore that backup.
Everything written after the backup is lost, so put the site in offline mode first if you need to
copy anything out.

### Upgrading PostgreSQL

A backup does not depend on the database version, so it is also the way to a new PostgreSQL major
version (the repository's compose files use 18):

1. Take a backup and copy it somewhere safe.
2. Start a new, empty database on the new version. The official `postgres:18` image keeps its data
   one folder deeper (mount the volume at `/var/lib/postgresql`, not `.../data`), so it will not
   start on a volume that holds an older version's files: use a new volume and keep the old one
   until you are sure.
3. Start the forum once so it migrates the empty database, stop it, then run
   `boring-forum restore <backup> --yes` (or restore from the admin panel).

### Prove it works

An untested backup is a guess. Restore one on another machine now and then and look at it. CI does
the same weekly with a scratch database: it backs up a migrated database, changes it, restores the
backup and compares every table row by row (`quality.yml`, job `backup-drill`; locally,
`BF_DATABASE_URL=… cargo test -p bf-server a_backup_restores -- --ignored` against an empty
disposable database).

### The secret key is part of the backup

`BF_SECRET_KEY` is deliberately **not** in the database or in these backups. A restore without it
leaves TOTP and webhook secrets unreadable, so members with two-factor sign-in are locked out
and webhooks stop signing. Store the current key (and any `BF_SECRET_KEY_OLD`) in a password
manager or secrets vault that is not on the same machine, and write down who can reach it. After
a restore, set the same key before the first start, then run `boring-forum --check-config`.

## Importing a forum

### With Docker: one command

The compose file has an `importer` service (profile `import`). It starts a throwaway database inside
its own container, restores your backup into it, exports it and imports it into the forum. Nothing of
the old forum is touched, and re-running skips what is already there.

```sh
# Discourse: give it the backup .tar.gz (its uploads are inside)
docker compose --profile import run --rm -v "$PWD/backup:/in:ro" importer discourse /in/backup.tar.gz

# Everything else: a SQL dump of the old database, and the old site's folder for attachments
docker compose --profile import run --rm -v "$PWD/old:/in:ro" importer phpbb /in/forum.sql.gz --files /in/phpbb
```

| Source | Backup you give | `--files` (optional: attachments, avatars) |
| --- | --- | --- |
| `discourse` | the backup `.tar.gz` | not needed |
| `phpbb` | SQL dump | the phpBB folder (with `files/`, `images/`) |
| `mybb` | SQL dump | the MyBB folder (with `uploads/`) |
| `ips` | SQL dump | the uploads folder |
| `vbulletin` | SQL dump (4.x; 5.x is not supported) | the vBulletin folder (with `attachments/`, `customavatars/`) |
| `xenforo` | SQL dump | the XenForo folder (with `internal_data/`) |
| `smf` | SQL dump | the SMF folder (with `attachments/`) |
| `fluxbb` (and PunBB) | SQL dump | none (no attachments in core) |
| `flarum` | SQL dump | the Flarum folder (with `assets/`) |
| `bbpress` | SQL dump of the WordPress database | none |

Options: `--dry-run` exports and counts without importing, `--public` lets guests read what came in
(otherwise members only), `--prefix P` sets the table prefix when it is not found automatically,
`--db URL` exports from a database that is already running instead of restoring a dump.
`docker compose --profile import run --rm importer` with no arguments prints this list.

The exporters are written from each product's published schema. Try the import against a copy
first (`docker compose -p try ...`), look at the forums, members and a few long topics, then run it
for real. Passwords carry over for Discourse, IPS, MyBB, phpBB, Flarum, XenForo (bcrypt) and bbPress; the old hash is checked at the member's next sign-in and replaced. Members of vBulletin, SMF and FluxBB/PunBB reset their password. The vBulletin, XenForo, SMF, FluxBB, Flarum and bbPress exporters have not yet been run against real installs: expect to adjust them, and report what a real forum shows.

### By hand

1. Export with the matching `tools/importers/export-<source>.mjs` (`ips`, `phpbb`, `mybb`, `discourse`, `vbulletin`, `xenforo`, `smf`, `fluxbb`, `flarum`, `bbpress`). Each needs its database driver installed (`npm i pg` or `npm i mysql2`) and only reads the source. For a Discourse backup (`.tar.gz`), restore its `dump.sql.gz` into a scratch Postgres database and export from that, never from the live site; unpack its `uploads/` folder next to it.
2. Run `boring-forum import out.json` against a copy first. Running it again skips what was imported.

| Variable | Default | |
| --- | --- | --- |
| `BF_IMPORT_FILES` | unset | The folder the export's `uploads` section names its files in (a Discourse backup's `uploads/`). Each file is stored like a member's upload, without their daily limit, and links to it in posts (`upload://…`, the old `/uploads/default/…` addresses) are rewritten, so pictures show with their size and open in the viewer. Avatars are set. Unset, the files are skipped and those links stay as they were. |
| `BF_IMPORT_PUBLIC` | unset | `1` lets guests read every imported forum that does not say `public` |
| `BF_IMPORT_PLUGINS` | on | `0` skips plugin hooks while importing |

IPS, phpBB and Discourse members keep their passwords, which are upgraded at their next sign-in. For phpBB, give the importer the phpBB folder (the one holding `files/` and `images/`) as `BF_IMPORT_FILES`: attachments and uploaded avatars are copied from it, and forums only staff can read are left out. For MyBB, `BF_IMPORT_FILES` is the MyBB folder (the one holding `uploads/`); its password hashes are checked and replaced at first sign-in, and forums guests and members cannot read forums are left out. `tools/importers/fixtures/` rebuilds phpBB and MyBB test installs to try the exports on.

Imports cover members (with their trust level, and the days they visited and topics they read, which count toward the levels here) and their avatars, forums (in the source's order), topics (with how many times each was viewed), posts, tags, groups, polls, reactions, private messages between people and the files posts use. A file this site does not accept (its kind or size) is skipped and counted. Permissions are not imported: forums get the default permissions, so check private forums before opening the site. Imported members never become staff; promote the accounts that should be, in the admin panel or with `UPDATE users SET primary_group_id = 4 WHERE username_lower = '…'`.
