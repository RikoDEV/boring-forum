#!/usr/bin/env bash
# Quick check of the main journeys against a server on a FRESH database: tests/http/smoke.sh [base-url]
set -u
B=${1:-http://127.0.0.1:8099}
J=$(mktemp); K=$(mktemp); fail=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1 (got '$2', want '$3')"; fail=1; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "ok   $1"; else echo "FAIL $1 (missing '$3')"; fail=1; fi; }
csrf() { grep -oE 'bf_csrf\s+[0-9a-f]{64}' "$1" | awk '{print $2}' | tail -1; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

curl -s -c $J -b $J -o /dev/null $B/register; T=$(csrf $J)
check "register admin" "$(code -c $J -b $J --data "csrf=$T&username=alice&email=a@example.com&password=correct horse battery" $B/register)" 303
A=(-b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json')
check "create category" "$(code "${A[@]}" -d '{"kind":"category","title":"General"}' $B/api/v1/admin/nodes)" 201
check "create forum" "$(code "${A[@]}" -d '{"kind":"forum","title":"Intro","parent_id":1}' $B/api/v1/admin/nodes)" 201
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Hello smoke topic" --data-urlencode "body=First post with **bold** text in it" $B/c/2/new)
has "topic created" "$LOC" "/t/hello-smoke-topic/1"
TID=1
check "reply" "$(code -b $J --data-urlencode "csrf=$T" --data-urlencode "body=A reply with enough text" $B/t/$TID/reply)" 303
sleep 0.5
TP=$(curl -s "$LOC")
has "topic page renders post" "$TP" '<strong>bold</strong>'
has "topic has author panel" "$TP" 'bf-author'
HP=$(curl -s $B/)
has "home lists forum" "$HP" 'bf-forum-row'
has "home shows last topic" "$HP" 'Hello smoke topic'
check "forum page" "$(code -L $B/c/2)" 200
check "latest" "$(code $B/latest)" 200
check "search page" "$(code "$B/search?q=smoke")" 200
check "profile" "$(code -L $B/u/alice)" 200
has "signed-in user menu" "$(curl -s -b $J $B/)" '<a class="nav-user" href="/settings">alice</a>'
check "settings" "$(code -b $J $B/settings)" 200
check "styleguide for staff" "$(code -b $J $B/styleguide)" 200
check "styleguide closed to guests" "$(code $B/styleguide)" 403
ICON=$(grep -oE '/assets/icons\.[0-9a-f]+\.svg' <<<"$HP" | head -1)
check "icon sprite" "$(code $B$ICON)" 200
check "admin panel" "$(code -L $B/admin/)" 200
exit $fail
