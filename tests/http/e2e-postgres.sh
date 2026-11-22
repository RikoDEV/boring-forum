#!/usr/bin/env bash
# Runs e2e against the WSL compose Postgres on a fresh database.
# Usage: tests/http/e2e-postgres.sh [suite-script]; the suite is tests/http/e2e.sh unless given.
# Normally started by scripts/test.sh. Outside it, needs the compose database up; on Windows
# set W to the WSL prefix, as for query-count.sh.
SUITE=${1:-tests/http/e2e.sh}
DBHOST=${BF_TEST_DB_HOST:-127.0.0.1}
# Server timers run at this percentage of normal; e2e.sh shortens its waits to match.
export BF_TIME_SCALE_PERCENT=${BF_TIME_SCALE_PERCENT:-10}
export BF_RATE_LIMIT_PER_MIN=${BF_RATE_LIMIT_PER_MIN:-1000000}
cd "$(dirname "$0")/../.."
S=$(mktemp -d)
W=${W:-}
if [ -n "${BF_TEST_DB_HOST:-}" ]; then psql -q -h "$DBHOST" -U forum -d forum -c 'DROP DATABASE IF EXISTS forum_e2e WITH (FORCE)' -c 'CREATE DATABASE forum_e2e'
else $W docker compose -f deploy/compose.yml exec -T db psql -q -U forum -d forum -c 'DROP DATABASE IF EXISTS forum_e2e WITH (FORCE)' -c 'CREATE DATABASE forum_e2e'; fi
[ -n "${BF_TEST_DB_HOST:-}" ] && cargo build -q -p bf-server --bin boring-forum
BIN=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' | sed 's#\\#/#g')/debug/boring-forum
export BF_MAIL_DIR=$(mktemp -d) BF_UPLOAD_DIR=$(mktemp -d)
export BF_GEO_HEADER=X-Test-Country BF_ASN_HEADER=X-Test-ASN BF_BOUNCE_TOKEN=0123456789abcdef0123456789abcdef
# The outside logins (SSO, OIDC, Apple) are site settings now: their checks run in the vitest suite
# (tests/e2e), which sets them up, not here.
export OIDC=http://127.0.0.1:9911
export BF_VAPID_PRIVATE=yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw BF_TICK_SECONDS=1 BF_DELETE_GRACE_DAYS=0 BF_REPLY_DOMAIN=mail.example BF_INBOUND_TOKEN=abcdef0123456789abcdef0123456789
export BF_WEBHOOK_ALLOW_LOOPBACK=1
export BF_CAPTCHA_VERIFY_URL=$OIDC/siteverify
export BF_OAUTH_APPLE_BASE=$OIDC
node tests/support/mock-oidc.mjs 9911 >/dev/null & M=$!
BF_BIND=127.0.0.1:8099 BF_DATABASE_URL="postgres://forum:forum@$DBHOST:5432/forum_e2e?sslmode=disable" BF_ADMIN_DIR="$PWD/web/admin/build" RUST_LOG=warn NO_COLOR=1 "$BIN" >$S/server.log 2>&1 & P=$!
curl -s --retry 60 --retry-delay 1 --retry-connrefused -o /dev/null http://127.0.0.1:8099/healthz
bash "$SUITE" http://127.0.0.1:8099 > $S/e2e.out 2>&1; echo "e2e exit $?"
kill $P $M 2>/dev/null
grep -c '^ok' $S/e2e.out; grep -v '^ok' $S/e2e.out | head -40; ! grep -q '^FAIL' $S/e2e.out
