# Troubleshooting

Start with the logs and `/readyz`. The server logs to standard output (`BF_LOG_JSON=true` gives one
JSON object per line, which the container image sets; `RUST_LOG` changes the level, for example
`RUST_LOG=debug,sqlx=warn`). `boring-forum --check-config` reports configuration problems without
starting the server.

## The server will not start

**`fatal: migration 1 was previously applied but has been modified`** (or any migration number).
The database was migrated by a different version of the forum than the one now running. Do not
edit the database's migration table by hand. Run the version that matches the database, or restore
a backup into an empty database ([Operations](operations.md#restoring)).

**`pool timed out while waiting for an open connection`** or the server exits straight away. The
database is not reachable: it is stopped, the address in `BF_DATABASE_URL` is wrong, or a
firewall is in the way. Once the database is back, a running server reconnects by itself.

**An error naming a variable.** `BF_ADMIN_TOKEN` shorter than 32 characters, `BF_TLS_CERT`
without `BF_TLS_KEY`, an unparsable number, and similar mistakes stop the server with the name of
the variable. Fix it and start again.

**`BF_DATABASE_URL connects as a Postgres SUPERUSER`.** Not a failure, but a warning to act on
before a real launch: run as a role that cannot change the schema
([Configuration](configuration.md#database-roles)).

## `/readyz` says unavailable

`/readyz` returns a JSON body with four flags:

| Flag | If false |
| --- | --- |
| `db` | the database did not answer within two seconds |
| `migrations` | the database has fewer migrations than this server expects |
| `config` | `BF_BASE_URL` is empty |
| `draining` | true while the server shuts down, which is normal during a restart |

`/livez` only says the process is up, and is the one to use for "restart if dead".

## The site is up but something is off

**Everyone is rate limited, or every visitor has the same address.** The server is behind a proxy
and does not know it. Set `BF_TRUSTED_PROXIES` to the proxy's address so the forwarded client
address is believed ([Installing](installation.md#a-reverse-proxy-and-tls)).

**A redirect loop, or login that never sticks, behind a proxy.** The server decides about HTTPS from
`BF_BASE_URL` and the forwarded protocol. Use the public `https://` address for `BF_BASE_URL`,
trust the proxy, and let it send `X-Forwarded-Proto`. Sessions use `__Host-` cookies, which
browsers send only over HTTPS, so a public address served over plain `http` will not keep
anyone signed in.

**`/admin` shows "not found".** That is what everyone but an administrator sees, on purpose. If you
are an administrator and it is still missing, the admin panel's files are not where the server
looks: build it (`pnpm build` in `web/`) or point `BF_ADMIN_DIR` at it. The container image has it
at `/admin`.

**Live updates arrive late or not at all.** A proxy is buffering responses. Turn response buffering
off for the site.

**Pictures and uploads fail.** The server needs to write to `BF_UPLOAD_DIR`. In the container
image it runs as user 65534 on a read-only filesystem, so the uploads folder must be the
volume. If you mount your own directory, make that user the owner.

**Pages show old content for a couple of seconds.** Guests are served from a short in-process cache
(`BF_MICROCACHE_TTL_MS`, 2000 by default). Signed-in members are not. **Admin, Maintenance, Clear
caches** empties it at once.

## Email

Open **Admin, Mail**. Each message is listed with its state; failed ones are retried up to five
times and then left there with the error.

- Nothing at all is queued: the forum is not sending, or the event did not happen. Check the
  address on the account and the member's notification settings.
- Messages are queued but fail: `BF_SMTP_URL` or `BF_MAIL_FROM` is wrong. Use `smtps://` for port
  465 and the provider's exact credentials.
- Messages are sent but land in spam: set up SPF, DKIM and DMARC for the sender's domain. The
  forum cannot do that for you.
- To see mail without sending it while developing, set `BF_MAIL_DIR` and each message is written
  to a file there.

## Backups

**Restore is refused with "from a newer version".** The backup was made by a newer release than the
one running. Update the server first, then restore.

**Restore fails with a permission error.** The role must own the tables. Set
`BF_MIGRATE_DATABASE_URL` to the schema-owning role and the restore uses it.

**A backup you copied in is not listed.** It must be in `BF_BACKUP_DIR` (`/data/backups` in the
container), keep its `boring-forum-….tar.gz` name and be a real backup: a file whose manifest
cannot be read is left out.

**Everyone was signed out after a restore.** Expected: sessions are part of what is restored.

**Two-factor sign-in stopped working after moving to a new server.** `BF_SECRET_KEY` on the new
server is not the one the old one used ([Operations](operations.md#the-secret-key-is-part-of-the-backup)).

## Still stuck

Search the [issues](https://github.com/RikoDEV/boring-forum/issues), and if it is new, open one
with the version, how you run it, and the relevant log lines with secrets removed. Report anything
that looks like a security problem privately, as described in the
[security policy](https://github.com/RikoDEV/boring-forum/blob/main/SECURITY.md).
