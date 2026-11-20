#!/usr/bin/env bash
# Imports another forum's backup into boring-forum, in one step. Runs inside the `importer` image:
#
#   docker compose --profile import run --rm -v "$PWD/backup:/in:ro" importer discourse /in/backup.tar.gz
#
# It starts a throwaway database inside the container, restores the backup into it, exports it to
# the neutral JSON format with tools/importers/export-<source>.mjs, and runs `boring-forum import`
# against the real forum database (BF_DATABASE_URL, as for the server). The scratch database goes
# away with the container; nothing of the old forum is changed.
set -euo pipefail

SOURCES="discourse phpbb mybb ips vbulletin xenforo smf fluxbb flarum bbpress"
usage() {
  cat >&2 <<EOF
Usage: import-forum <source> <backup> [options]

  source   one of: $SOURCES
  backup   discourse: the backup .tar.gz (or the folder it was unpacked to)
           others:    a SQL dump (.sql or .sql.gz) of the forum's database
           (with --db the backup is not needed)

Options:
  --files DIR    the old forum's folder with its uploads (phpBB: the folder with files/ and images/;
                 MyBB: with uploads/; XenForo: with internal_data/; SMF: with attachments/; Flarum: with assets/).
                 Discourse needs none: its uploads are inside the backup. Without it, attachments
                 and avatars are skipped.
  --prefix P     table prefix, when it is not found automatically
  --db URL       export from a database that is already running (mysql://... or postgres://...)
                 instead of restoring a dump
  --public       let guests read the imported forums (default: members only until you decide)
  --dry-run      export and count only; nothing is written to the forum
EOF
  exit 2
}

[ $# -ge 1 ] || usage
SOURCE=$1; shift
case " $SOURCES " in *" $SOURCE "*) ;; *) echo "Unknown source '$SOURCE'." >&2; usage ;; esac
BACKUP=""; FILES=""; PREFIX=""; DB_URL=""; PUBLIC=0; DRY=0
if [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; then BACKUP=$1; shift; fi
while [ $# -gt 0 ]; do
  case $1 in
    --files) FILES=${2:?--files needs a folder}; shift 2 ;;
    --prefix) PREFIX=${2:?--prefix needs a value}; shift 2 ;;
    --db) DB_URL=${2:?--db needs a URL}; shift 2 ;;
    --public) PUBLIC=1; shift ;;
    --dry-run) DRY=1; shift ;;
    *) echo "Unknown option $1." >&2; usage ;;
  esac
done
[ -n "$BACKUP" ] || [ -n "$DB_URL" ] || { echo "Give the backup to import." >&2; usage; }
if [ "$DRY" = 0 ] && [ -z "${BF_DATABASE_URL:-}" ]; then
  echo "BF_DATABASE_URL is not set: run this through docker compose (the importer service sets it)." >&2
  exit 1
fi

say() { printf '\n==> %s\n' "$*"; }
WORK=${IMPORT_WORK:-/work}
mkdir -p "$WORK"
upper=$(echo "$SOURCE" | tr a-z A-Z)

# A table every install of the software has, to read the table prefix from.
marker_of() {
  case $1 in
    phpbb|mybb) echo forums ;;
    ips) echo core_members ;;
    vbulletin|xenforo) echo thread ;;
    smf) echo messages ;;
    fluxbb) echo topics ;;
    flarum) echo discussions ;;
    bbpress) echo posts ;;
  esac
}

# ---- 1. the source database -------------------------------------------------------------------
if [ -n "$DB_URL" ]; then
  SRC_URL=$DB_URL
elif [ "$SOURCE" = discourse ]; then
  say "Unpacking the backup"
  if [ -d "$BACKUP" ]; then
    ROOT=$BACKUP
  else
    ROOT=$WORK/unpacked; rm -rf "$ROOT"; mkdir -p "$ROOT"
    tar -xzf "$BACKUP" -C "$ROOT"
  fi
  DUMP=$(find "$ROOT" -maxdepth 2 -name 'dump.sql*' | head -n1)
  [ -n "$DUMP" ] || { echo "No dump.sql.gz in the backup: is it a Discourse backup?" >&2; exit 1; }
  if [ -z "$FILES" ] && [ -d "$ROOT/uploads" ]; then FILES=$ROOT/uploads; fi
  say "Starting a scratch Postgres and restoring the dump (this can take a while)"
  PGDATA=$WORK/pg; rm -rf "$PGDATA"; mkdir -p "$PGDATA" /run/postgresql
  chown postgres:postgres "$PGDATA" /run/postgresql
  su-exec postgres initdb -D "$PGDATA" -A trust >/dev/null
  su-exec postgres pg_ctl -D "$PGDATA" -o "-c listen_addresses=127.0.0.1 -c fsync=off -c synchronous_commit=off" -w start >/dev/null
  # The dump names its owner and may add extensions the scratch server lacks: errors there are fine.
  psql -q -U postgres -h 127.0.0.1 -c "CREATE ROLE discourse SUPERUSER LOGIN" -c "CREATE DATABASE discourse OWNER discourse" >/dev/null
  case $DUMP in *.gz) gunzip -c "$DUMP" ;; *) cat "$DUMP" ;; esac \
    | psql -q -U postgres -h 127.0.0.1 -d discourse -v ON_ERROR_STOP=0 >"$WORK/restore.log" 2>&1 || true
  SRC_URL=postgres://postgres@127.0.0.1/discourse
else
  [ -f "$BACKUP" ] || { echo "$BACKUP is not a file. Give a .sql or .sql.gz dump." >&2; exit 1; }
  say "Starting a scratch MariaDB and restoring the dump (this can take a while)"
  MYDATA=$WORK/mysql; rm -rf "$MYDATA"; mkdir -p "$MYDATA" /run/mysqld
  chown mysql:mysql "$MYDATA" /run/mysqld
  mariadb-install-db --user=mysql --datadir="$MYDATA" --skip-test-db >/dev/null
  su-exec mysql mariadbd --datadir="$MYDATA" --skip-networking=OFF --bind-address=127.0.0.1 --innodb-flush-log-at-trx-commit=0 \
    --max-allowed-packet=256M --character-set-server=utf8mb4 >"$WORK/mariadb.log" 2>&1 &
  for _ in $(seq 60); do mariadb -uroot -e 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done
  mariadb -uroot -e "CREATE DATABASE src CHARACTER SET utf8mb4; CREATE USER 'imp'@'%' IDENTIFIED BY 'imp'; GRANT ALL ON *.* TO 'imp'@'%'"
  case $BACKUP in *.gz) gunzip -c "$BACKUP" ;; *) cat "$BACKUP" ;; esac \
    | mariadb -uroot --force --default-character-set=utf8mb4 src >"$WORK/restore.log" 2>&1 || true
  # A dump may carry its own CREATE DATABASE: take the database that holds the tables.
  DBNAME=$(mariadb -uroot -N -e "SELECT table_schema FROM information_schema.tables WHERE table_schema NOT IN ('information_schema','mysql','performance_schema','sys') GROUP BY 1 ORDER BY count(*) DESC LIMIT 1")
  [ -n "$DBNAME" ] || { echo "The dump restored no tables; see $WORK/restore.log." >&2; exit 1; }
  if [ -z "$PREFIX" ]; then
    m=$(marker_of "$SOURCE")
    PREFIX=$(mariadb -uroot -N -e "SELECT table_name FROM information_schema.tables WHERE table_schema='$DBNAME' AND table_name LIKE '%$m' ORDER BY length(table_name) LIMIT 1")
    PREFIX=${PREFIX%"$m"}
    echo "Table prefix: '${PREFIX}'"
  fi
  SRC_URL=mysql://imp:imp@127.0.0.1/$DBNAME
fi

# ---- 2. export ----------------------------------------------------------------------------------
say "Exporting to the neutral format"
OUT=$WORK/out.json
export "${upper}_DB=$SRC_URL"
if [ -n "$PREFIX" ]; then export "${upper}_PREFIX=$PREFIX"; fi
node "/opt/importers/export-$SOURCE.mjs" "$OUT"
if [ -n "$FILES" ]; then
  export BF_IMPORT_FILES=$FILES
  echo "Files from $FILES"
else
  echo "No files folder: attachments and avatars are skipped (--files DIR)."
fi

# ---- 3. import ----------------------------------------------------------------------------------
if [ "$DRY" = 1 ]; then
  say "Dry run: nothing imported. The export is $OUT (in the importwork volume)."
  exit 0
fi
say "Importing into the forum"
if [ "$PUBLIC" = 1 ]; then export BF_IMPORT_PUBLIC=1; fi
export BF_UPLOAD_DIR=/data/uploads
(cd / && /boring-forum import "$OUT")
# The server runs as nobody and must read what the importer (root) stored.
chown -R 65534:65534 /data/uploads 2>/dev/null || true
rm -f "$OUT"
cat <<'EOF'

Done. Next:
  - Imported members are never staff: promote your admin with
      docker compose exec db psql -U forum -d forum -c "UPDATE users SET primary_group_id = 4 WHERE username_lower = 'yourname'"
  - Permissions are not imported: check private forums before opening the site.
  - Running the import again skips what is already there.
EOF
