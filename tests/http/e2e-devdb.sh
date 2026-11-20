#!/usr/bin/env bash
# Starts a fresh PGlite devdb and the server, runs the Postgres tests and the e2e
# smoke test, then stops both. Needs `pnpm install` in tools/devdb and a built
# server binary (`cargo build -p bf-server`); pass its path if not the default.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${1:-$(cargo metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' | sed 's#\\\\#/#g')/debug/boring-forum}
DB_PORT=${BF_DEVDB_PORT:-54329}
WEB_PORT=8099
URL="postgres://postgres@127.0.0.1:$DB_PORT/postgres?sslmode=disable"
pids=()
cleanup() { kill "${pids[@]}" 2>/dev/null; wait 2>/dev/null; }
trap cleanup EXIT

start_db() {
  (cd "$ROOT/tools/devdb" && exec node server.mjs $DB_PORT) >/dev/null &
  pids+=($!)
  for _ in $(seq 50); do (exec 3<>/dev/tcp/127.0.0.1/$DB_PORT) 2>/dev/null && return; sleep 0.2; done
  echo "devdb did not start"; exit 1
}

start_db
echo "== Postgres tests"
BF_DEVDB=1 BF_DATABASE_URL=$URL cargo test -q -p bf-db -- --ignored --test-threads=1 2>&1 | grep -E "test result: .* [1-9][0-9]* (passed|failed)|FAILED|panicked"
kill "${pids[@]}"; wait 2>/dev/null; pids=()

start_db
export BF_MAIL_DIR=$(mktemp -d) BF_UPLOAD_DIR=$(mktemp -d) BF_BOUNCE_TOKEN=0123456789abcdef0123456789abcdef
OIDC_PORT=9911
export OIDC=http://127.0.0.1:$OIDC_PORT
export BF_WEBHOOK_ALLOW_LOOPBACK=1 BF_SSO_URL=$OIDC/sso BF_SSO_SECRET=0123456789abcdef0123456789abcdef
export BF_CAPTCHA_SECRET=s BF_CAPTCHA_VERIFY_URL=$OIDC/siteverify
export BF_OAUTH_APPLE_ID=com.example.forum BF_OAUTH_APPLE_TEAM_ID=TEAM BF_OAUTH_APPLE_KEY_ID=KID BF_OAUTH_APPLE_BASE=$OIDC
export BF_OAUTH_APPLE_KEY="$(node -e 'console.log(require("crypto").generateKeyPairSync("ec",{namedCurve:"P-256"}).privateKey.export({type:"pkcs8",format:"pem"}))')" BF_OIDC_NAME="Test SSO" BF_OIDC_CLIENT_ID=test-client BF_OIDC_CLIENT_SECRET=test-secret
export BF_OIDC_AUTHORIZE_URL=$OIDC/authorize BF_OIDC_TOKEN_URL=$OIDC/token BF_OIDC_USERINFO_URL=$OIDC/userinfo
node "$ROOT/tests/support/mock-oidc.mjs" $OIDC_PORT >/dev/null &
pids+=($!)
BF_DEVDB=1 BF_DB_POOL_SIZE=1 BF_BIND=127.0.0.1:$WEB_PORT BF_DATABASE_URL=$URL BF_ADMIN_DIR="$ROOT/web/admin/build" RUST_LOG=warn "$BIN" &
pids+=($!)
curl -s --retry 60 --retry-delay 1 --retry-connrefused -o /dev/null "http://127.0.0.1:$WEB_PORT/healthz"
echo "== e2e"
bash "$ROOT/tests/http/e2e.sh" "http://127.0.0.1:$WEB_PORT"
status=$?
echo "== e2e exit $status"
exit $status
