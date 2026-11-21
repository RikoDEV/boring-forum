#!/usr/bin/env bash
# Fails when a page runs more SQL statements than its budget. The point is that the count stays flat
# as a page holds more rows (a query per row would blow the budget), not that it is tiny: a list page
# for a guest needs a handful, a member's page adds the session user, notifications, widgets and, for
# staff, the review badge (the check signs in as the administrator). Budgets are the measured count
# plus about two, set on 2026-11-21; raise one only with the reason in the commit message.
# Needs the compose database and a built server. Turns on statement logging for a scratch database and counts log lines around
# each request, on a warm cache. On Windows set W to the WSL prefix:
#   W="env MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu --cd /mnt/c/path/to/boring-forum --" scripts/query-count.sh
cd "$(dirname "$0")/.."
W=${W:-}
fail=0
psql() { $W docker compose -f deploy/compose.yml exec -T db psql -q -U forum -d "$1" -c "$2" 2>&1 | tr -d '\000'; }
psql forum 'DROP DATABASE IF EXISTS forum_qc WITH (FORCE)'
psql forum 'CREATE DATABASE forum_qc'
psql forum "ALTER DATABASE forum_qc SET log_statement = 'all'"
BIN=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' | sed 's#\\\\#/#g')/debug/boring-forum
export BF_MAIL_DIR=$(mktemp -d) BF_UPLOAD_DIR=$(mktemp -d)
BF_BIND=127.0.0.1:18097 BF_DATABASE_URL="postgres://forum:forum@127.0.0.1:5432/forum_qc?sslmode=disable" BF_ADMIN_DIR="$PWD/web/admin/build" BF_MICROCACHE_TTL_MS=0 RUST_LOG=error "$BIN" >/dev/null 2>&1 & P=$!
B=http://127.0.0.1:18097
curl -sf --retry 60 --retry-delay 1 --retry-connrefused -o /dev/null $B/healthz || { echo "server did not become healthy"; exit 1; }
J=$(mktemp)
curl -s -c $J -b $J -o /dev/null $B/register
T=$(grep -oE 'bf_csrf\s+[0-9a-f]{64}' $J | awk '{print $2}' | tail -1)
curl -s -o /dev/null -c $J -b $J --data "csrf=$T&username=alice&email=a@example.com&password=correct horse battery" $B/register
curl -s -o /dev/null -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"category","title":"General"}' $B/api/v1/admin/nodes
curl -s -o /dev/null -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"forum","title":"Intro","parent_id":1}' $B/api/v1/admin/nodes
TF=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode "title=Hello there world" --data-urlencode "body=Some body text that is long enough" --data-urlencode "tags=a,b" $B/c/2/new)
for i in 1 2 3; do curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode "body=A reply with enough text number $i" $(echo $TF | sed -E 's#/t/[^/]+/([0-9]+).*#/t/\1/reply#'); done
# More topics, a club, so a query run once per row shows as a
# count that grows with the page instead of hiding behind one row.
for n in 1 2 3 4 5; do
  curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Another topic number $n here" --data-urlencode "body=Body text for topic number $n, long enough" --data-urlencode "tags=a,c$n" $B/c/2/new
done
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode "name=Readers" --data-urlencode "description=A club for the check" $B/clubs
sleep 7
count() { # label, curl args
  local label=$1; shift
  curl -s -o /dev/null "$@"; curl -s -o /dev/null "$@"; sleep 1
  local since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  sleep 1
  curl -s -o /dev/null "$@"
  sleep 1
  local n=$($W docker compose -f deploy/compose.yml logs --since "$since" db 2>/dev/null | tr -d '\000' | grep -E 'statement: |execute ' | grep -vE 'mail_outbox|webhook_deliveries|sessions|item_views|view_count|LISTEN|last_seen' > /tmp/qc.txt; wc -l < /tmp/qc.txt)
  echo "$label: $n statements"
  # A cold database can be 0 for one page; every page at 0 means the log was not read at all.
  seen=$((seen + n))
  if [ "$n" -gt "${BUDGET:-4}" ]; then fail=1; cut -d: -f4- /tmp/qc.txt | cut -c1-120; fi
}
count "guest home" $B/
count "guest forum" -L $B/c/2
BUDGET=7 count "guest topic" -L "$TF"
BUDGET=11 count "member home" -b $J $B/
BUDGET=8 count "member forum" -L -b $J $B/c/2
BUDGET=13 count "member topic" -L -b $J "$TF"
# The main pages, each with its own budget (the page needs more than the list pages); what
# matters is that it stays flat when the page holds several rows, so a query per row fails.
BUDGET=6 count "guest latest" -L $B/latest
BUDGET=9 count "member latest" -L -b $J $B/latest
BUDGET=20 count "member profile" -L -b $J $B/u/alice
BUDGET=12 count "guest profile" -L $B/u/alice
BUDGET=15 count "member search" -L -b $J "$B/search?q=topic"
BUDGET=11 count "guest search" -L "$B/search?q=topic"
BUDGET=9 count "member calendar" -L -b $J $B/calendar
BUDGET=6 count "guest calendar" -L $B/calendar
BUDGET=9 count "member clubs" -L -b $J $B/clubs
BUDGET=3 count "guest clubs" -L $B/clubs
kill $P
if [ "${seen:-0}" -eq 0 ]; then echo "no statements seen: the check measured nothing"; fail=1; fi
psql forum 'DROP DATABASE forum_qc WITH (FORCE)'
exit $fail
