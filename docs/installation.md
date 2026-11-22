# Installing Boring Forum

Boring Forum is one server binary and a PostgreSQL database. This page takes you from nothing to a
forum other people can use. For a quick local trial, the [README](https://github.com/RikoDEV/boring-forum#quick-start)
is shorter; for every setting see [Configuration](configuration.md).

## What you need

- **PostgreSQL 18.** The server creates its own tables when it starts. The database user needs
  to be able to create the `pg_trgm` and `unaccent` extensions, which Postgres allows the owner
  of a database to do.
- **A machine that can run the container image** (any Linux host with Docker), or a Rust toolchain
  to build the binary yourself.
- **A domain name and TLS** for a public site, from a reverse proxy or from the server itself.
- **An SMTP account** if you want the forum to send email: sign-up confirmation, password
  resets and notifications.

Node.js is only needed to build the front end from source; the container image already holds it.

## With Docker Compose

`deploy/compose.yml` runs Postgres and the published server image
(`ghcr.io/rikodev/boring-forum`) together, with volumes for the database, the uploaded files and
backups. It follows the newest release; set `BF_VERSION` (for example `0.0.1`) to stay on one.
To build the image from your checkout instead, add `-f deploy/compose.build.yml` and `--build`.
Postgres is not published to the host. For development, add `-f deploy/compose.dev.yml` to open it on `127.0.0.1:5432`.

You do not need the repository, only two files in an empty directory:

```sh
mkdir forum && cd forum
curl -fsSLO https://raw.githubusercontent.com/RikoDEV/boring-forum/main/deploy/compose.yml
curl -fsSL https://raw.githubusercontent.com/RikoDEV/boring-forum/main/.env.production -o .env
```

Edit `.env`: set `BF_BASE_URL` to the address people will use and `BF_SECRET_KEY` to the output of
`docker compose run --rm server secret-key`. Compose passes the whole file to the server, so every
setting in [Configuration](configuration.md) goes there. It always sets the database address and the
listen address itself, whatever `.env` says. `BF_VERSION=0.0.3` in the same file pins a release.

```sh
docker compose up -d --wait
```

From a checkout of the repository, use `deploy/compose.yml` the same way: put the `.env` in `deploy/`
and add `-f deploy/compose.yml` to each command.

The server listens on `127.0.0.1:8080` only, so nothing is reachable from outside until you put
a reverse proxy in front of it (below). Open the address, register, and **the first account on an
empty database becomes the administrator**. The admin panel is at `/admin` and is shown to
administrators only.

Keep `BF_SECRET_KEY` somewhere other than this machine. It encrypts two-factor secrets and webhook
secrets, and a restored backup is unreadable in those places without it
([Operations](operations.md#the-secret-key-is-part-of-the-backup)).

### The server does not run as the database owner

The compose file creates three least-privilege roles for you, so there is nothing to set up by hand.
A `db-roles` step runs the server image's own `provision-roles` command (it applies
`deploy/postgres/roles.sql`, built into the binary, and is safe to repeat) on every start, and the
server then migrates as `forum_migrate` and serves as `forum_app`, which can read and write rows but
cannot change the schema. A SQL injection that got through could not drop a table or reach the
database host. `forum_readonly` is there for ad-hoc queries.

Their passwords derive from `POSTGRES_PASSWORD` in `.env`, so setting that one value is enough.
`BF_APP_DB_PASSWORD`, `BF_MIGRATE_DB_PASSWORD` and `BF_READONLY_DB_PASSWORD` set them separately. The
background is in [Configuration](configuration.md#database-roles).

## Without Docker

Build and run the binary directly:

```sh
(cd web && pnpm install && pnpm build)          # island bundles and the admin panel
cargo build --release -p bf-server
export BF_DATABASE_URL=postgres://forum_app:…@127.0.0.1:5432/forum
export BF_BASE_URL=https://forum.example.com
export BF_SECRET_KEY=…                          # make it once: ./target/release/boring-forum secret-key
./target/release/boring-forum                   # migrations run at startup
```

An `https` site refuses to start without `BF_SECRET_KEY`. `boring-forum secret-key` prints a new one as
`BF_SECRET_KEY=…`; make it once, store it with your other secrets (a systemd `EnvironmentFile` works)
and keep a copy off this machine. A new key every start would leave the two-factor and webhook secrets
already stored unreadable.

The server finds the admin panel in `web/admin/build` (`BF_ADMIN_DIR` changes that) and stores
uploads and backups under `data/` (`BF_UPLOAD_DIR` and `BF_BACKUP_DIR`). Run it under a process
manager such as systemd so it restarts. It stops cleanly on `SIGTERM`.

## A reverse proxy and TLS

Either let a proxy terminate TLS, or let the server do it.

**Behind a proxy** (the usual choice). Set `BF_BASE_URL` to the public `https://` address and
`BF_TRUSTED_PROXIES` to the proxy's address, so the server believes the client address and
protocol the proxy forwards. Without it every visitor looks like the proxy and the per-address
rate limit is shared by all of them. With Caddy, for example:

```
forum.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Do not buffer responses: live updates use server-sent events, which a buffering proxy delays.

**Built-in TLS.** Set `BF_TLS_CERT` and `BF_TLS_KEY` to a PEM certificate chain and key, and
optionally `BF_HTTP_BIND=0.0.0.0:80` for a listener that only redirects to HTTPS. See
[TLS](configuration.md#tls).

## Email

Set `BF_SMTP_URL` and `BF_MAIL_FROM`:

```sh
BF_SMTP_URL=smtps://user:password@smtp.example.com:465
BF_MAIL_FROM="Example Forum <noreply@example.com>"
```

Outgoing mail is queued and retried. **Admin, Mail** shows what was sent and what failed, so that
is the first place to look if sign-up mails do not arrive. With no SMTP set the forum still works,
but nobody gets confirmation, reset or notification mail.

## Check the configuration

```sh
boring-forum --check-config
```

loads the environment the way the server would and reports errors and doubtful settings (a
bad value, a missing `BF_SECRET_KEY`, a plain-`http` base URL on a public address) without
starting. The server separately logs an error at start-up if it connects as a database superuser.
`/livez` answers while the process is up and `/readyz` once the database is reachable and
migrated; the container's health check uses `/readyz`.

## After the first start

1. Register your administrator account and turn on two-factor sign-in.
2. In **Admin, Settings**, set the site name, language and who may sign up.
3. Create your categories and forums under **Admin, Forums** ([Administration](administration.md)).
4. Send yourself a password reset to prove mail works.
5. Turn on automatic backups under **Admin, Backups** and copy the first one off the machine
   ([Operations](operations.md#backup-and-restore)).
6. Read the security settings in [Configuration](configuration.md) before you open sign-up.

## Upgrading

1. Take a backup (**Admin, Backups, Back up now**).
2. Pull the new image, or `git pull` and rebuild, and restart. The server applies new migrations
   when it starts.
3. Check `/readyz` and the admin panel.

Migrations only go forward. If an upgrade goes wrong, deploy the previous version against an
empty database and restore the backup ([Operations](operations.md#rolling-back-an-upgrade)).
Release notes are in the [release notes](https://github.com/RikoDEV/boring-forum/releases).
With Docker Compose the roles are re-applied on every `up`. Without it, run `deploy/postgres/roles.sql`
again after an upgrade so the new tables get their grants.
