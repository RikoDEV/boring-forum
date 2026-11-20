#!/usr/bin/env bash
# Runs a sample export through `boring-forum import` on an empty database named by
# BF_DATABASE_URL, then checks the result through a running server, including a bcrypt sign-in.
set -u
cd "$(dirname "$0")/../.."
BIN=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' | sed 's#\\\\#/#g')/debug/boring-forum
T=$(mktemp -d); B=http://127.0.0.1:18098; fail=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1 (got '$2', want '$3')"; fail=1; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "ok   $1"; else echo "FAIL $1 (missing '$3')"; fail=1; fi; }
cat > "$T/in.json" <<EOF
{"users":[{"id":7,"username":"oldalice","email":"oldalice@example.com","password_hash":"$(cat tools/importers/import-sample-hash.txt)","joined_at":"2015-03-01T10:00:00Z"},
 {"id":8,"username":"oldbob","email":"oldbob@example.com","password_hash":null,"joined_at":"2016-03-01T10:00:00Z"}],
 "forums":[{"id":11,"parent_id":10,"title":"Old Forum"},{"id":10,"parent_id":null,"title":"Old Category","kind":"category"}],
 "topics":[{"id":100,"forum_id":11,"title":"Imported topic","url":"/topic/100-imported-topic/"}],
 "posts":[{"id":1000,"topic_id":100,"author_id":7,"body":"<p>Hello <strong>old</strong> world</p><script>alert(1)</script>","format":"html","created_at":"2015-04-01T10:00:00Z"},
 {"id":1001,"topic_id":100,"author_id":8,"body":"[b]bold[/b] reply","format":"bbcode","created_at":"2015-04-02T10:00:00Z"},
 {"id":1002,"topic_id":100,"author_id":999,"body":"from a member who is gone","format":"markdown","created_at":"2015-04-03T10:00:00Z"}]}
EOF
has "first import" "$("$BIN" import "$T/in.json")" 'Imported 2 members, 2 forums, 1 topics and 3 posts'
has "second import adds nothing" "$("$BIN" import "$T/in.json")" 'Imported 0 members, 0 forums, 0 topics and 0 posts'
BF_BIND=127.0.0.1:18098 BF_MAIL_DIR="$T" BF_UPLOAD_DIR="$T" RUST_LOG=error "$BIN" >/dev/null 2>&1 & P=$!
curl -s --retry 60 --retry-delay 1 --retry-connrefused -o /dev/null $B/healthz
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' $B/topic/100-imported-topic/)
TP=$(curl -s -L $B/topic/100-imported-topic/)
has "old address leads to the topic" "$TP" 'Imported topic'
has "html converted" "$TP" '<strong>old</strong>'
has "bbcode converted" "$TP" '<strong>bold</strong>'
if grep -q '<script>alert' <<<"$TP"; then echo "FAIL script survived"; fail=1; else echo "ok   no script"; fi
has "date kept" "$TP" '2015-04-01'
has "missing member gets the placeholder" "$TP" 'imported'
J=$(mktemp); curl -s -c $J -b $J -o /dev/null $B/login; C=$(grep -oE 'bf_csrf\s+[0-9a-f]{64}' $J | awk '{print $2}' | tail -1)
check "wrong old password refused" "$(curl -s -o /dev/null -w '%{http_code}' -c $J -b $J --data-urlencode "csrf=$C" -d 'login=oldalice&password=wrong password here' $B/login)" 422
check "old password signs in" "$(curl -s -o /dev/null -w '%{http_code}' -c $J -b $J --data-urlencode "csrf=$C" -d 'login=oldalice&password=my old password' $B/login)" 303
J2=$(mktemp); curl -s -c $J2 -b $J2 -o /dev/null $B/login; C=$(grep -oE 'bf_csrf\s+[0-9a-f]{64}' $J2 | awk '{print $2}' | tail -1)
check "still works after upgrade to argon2" "$(curl -s -o /dev/null -w '%{http_code}' -c $J2 -b $J2 --data-urlencode "csrf=$C" -d 'login=oldalice&password=my old password' $B/login)" 303
kill $P
exit $fail
