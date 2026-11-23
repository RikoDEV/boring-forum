#!/usr/bin/env bash
# End-to-end smoke test against a running server on a FRESH database
# (it registers the first account, which becomes admin): tests/http/e2e.sh [base-url]
set -u
B=${1:-http://127.0.0.1:8099}
J=$(mktemp)   # admin cookie jar
K=$(mktemp)   # second user cookie jar
fail=0
# Waits out a server timer. The runner shortens the timers by the same factor.
settle() { sleep "$(awk -v s="$1" -v p="${BF_TIME_SCALE_PERCENT:-100}" 'BEGIN { printf "%.2f", s * p / 100 }')"; }
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1 (got '$2', want '$3')"; fail=1; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "ok   $1"; else echo "FAIL $1 (missing '$3')"; fail=1; fi; }
csrf() { grep -oE 'bf_csrf\s+[0-9a-f]{64}' "$1" | awk '{print $2}' | tail -1; }

MAIL=${BF_MAIL_DIR:-}
# Newest mail to $1 containing $2; prints the matching link.
mail_link() { [ -n "$MAIL" ] || return; for _ in $(seq 30); do f=$(grep -l -- "To: $1" "$MAIL"/*.eml 2>/dev/null | sort | tail -1); if [ -n "$f" ] && grep -q -- "$2" "$f"; then grep -oE "https?://[^ ]*$2[0-9a-f]{64}" "$f" | tail -1; return; fi; sleep 0.5; done; }
# TOTP code for base32 secret $1 at now + $2 seconds.
totp() { node -e '
const [s, off] = [process.argv[1], +process.argv[2]];
const A = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"; let bits = "";
for (const c of s) bits += A.indexOf(c).toString(2).padStart(5, "0");
const key = Buffer.from(bits.match(/.{8}/g).map(b => parseInt(b, 2)));
const ctr = Buffer.alloc(8); ctr.writeBigUInt64BE(BigInt(Math.floor((Date.now() / 1000 + off) / 30)));
const h = require("crypto").createHmac("sha1", key).update(ctr).digest(); const o = h[19] & 15;
console.log(String((h.readUInt32BE(o) & 0x7fffffff) % 1e6).padStart(6, "0"));' "$1" "$2"; }

check "healthz" "$(curl -s -o /dev/null -w '%{http_code}' $B/healthz)" 200

curl -s -c $J -b $J $B/register >/dev/null
T=$(csrf $J)
check "register admin" "$(curl -s -o /dev/null -w '%{http_code}' -c $J -b $J --data "csrf=$T&username=alice&email=alice@example.com&password=correct horse battery" $B/register)" 303
has "home shows user" "$(curl -s -b $J $B/)" "alice"

check "create category" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"category","title":"General"}' $B/api/v1/admin/nodes)" 201
FORUM=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"forum","title":"Introductions","parent_id":1,"description":"Say **hi**"}' $B/api/v1/admin/nodes | grep -oE '[0-9]+')
check "create forum" "$FORUM" 2

curl -s -c $K -b $K $B/register >/dev/null
T2=$(csrf $K)
check "register bob" "$(curl -s -o /dev/null -w '%{http_code}' -c $K -b $K --data "csrf=$T2&username=bob&email=bob@example.com&password=another long passphrase" $B/register)" 303

LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Hello from Alice" --data-urlencode "body=First post, hi @bob! **bold** <script>alert(1)</script>" $B/forum/$FORUM/new)
has "new topic redirects to topic" "$LOC" "/topic/1-hello-from-alice/"

TOPIC=$(curl -s $B/topic/1-hello-from-alice/)
has "guest sees topic" "$TOPIC" "Hello from Alice"
has "cooked bold" "$TOPIC" "<strong>bold</strong>"
if grep -q "<script>alert" <<<"$TOPIC"; then echo "FAIL script tag rendered"; fail=1; else echo "ok   script tag not rendered"; fi
has "json-ld posting" "$TOPIC" "DiscussionForumPosting"
has "canonical" "$TOPIC" 'rel="canonical" href="http://127.0.0.1:8099/topic/1-hello-from-alice/"'
check "wrong slug redirects" "$(curl -s -o /dev/null -w '%{http_code}' $B/topic/1-wrong/)" 308

check "bob cannot create forums" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -H "X-CSRF-Token: $T2" -H 'Content-Type: application/json' -d '{"kind":"forum","title":"x"}' $B/api/v1/admin/nodes)" 403
has "bob was mentioned" "$(curl -s -b $K $B/notifications)" "alice mentioned you in Hello from Alice"
check "bob replies" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&body=Hi Alice, welcome aboard!" $B/topic/1/reply)" 303
check "reply without csrf rejected" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=&body=Hi Alice, welcome aboard!" $B/topic/1/reply)" 400
has "alice got reply notification" "$(curl -s -b $J $B/notifications)" "bob posted in Hello from Alice"
check "bob cannot pin" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&action=pin" $B/topic/1/mod)" 403

check "alice locks topic" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&action=lock" $B/topic/1/mod)" 303
check "bob cannot reply to locked" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&body=Another reply here" $B/topic/1/reply)" 403
has "lock action entry shown" "$(curl -s -b $J $B/topic/1-hello-from-alice/)" "locked this topic"

has "forum lists topic" "$(curl -s $B/forum/2-introductions/)" "Hello from Alice"
has "home lists forum" "$(curl -s $B/)" "Introductions"
has "search finds reply" "$(curl -s -b $J "$B/search?q=welcome")" "Hello from Alice"
has "sitemap has topic" "$(curl -s $B/sitemap.xml)" "/topic/1-hello-from-alice/"
has "rss has topic" "$(curl -s $B/latest.rss)" "<title>Hello from Alice</title>"
has "preview cooks" "$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"raw":"*hi*"}' $B/api/v1/preview)" "<em>hi</em>"

check "mention link redirects to profile" "$(curl -s -o /dev/null -w '%{redirect_url}' $B/u/bob)" "$B/profile/2-bob/"
has "profile shows posts" "$(curl -s $B/profile/1-alice/)" "Hello from Alice"
check "profile wrong slug redirects" "$(curl -s -o /dev/null -w '%{http_code}' $B/profile/1-bob/)" 308
check "unknown profile 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/profile/999-x/)" 404
UN=$(curl -s "$B/forum/2-introductions/?show=unanswered")
if grep -q "Hello from Alice" <<<"$UN"; then echo "FAIL unanswered filter shows answered topic"; fail=1; else echo "ok   unanswered filter hides answered topic"; fi
has "sorted view is noindex" "$(curl -s "$B/forum/2-introductions/?sort=views")" 'name="robots" content="noindex"'
has "my topics for alice" "$(curl -s -b $J "$B/forum/2-introductions/?show=mine")" "Hello from Alice"

# --- watched words, approval queue, review
check "admin adds blocked word" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"action":"block","pattern":"badword"}' $B/api/v1/admin/watched-words)" 201
check "admin adds approval word" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"action":"require_approval","pattern":"casino"}' $B/api/v1/admin/watched-words)" 201
check "bob cannot add words" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -H "X-CSRF-Token: $T2" -H 'Content-Type: application/json' -d '{"action":"block","pattern":"x"}' $B/api/v1/admin/watched-words)" 403
check "blocked word rejected" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" --data-urlencode "title=A badword title here" --data-urlencode "body=Some long enough body text" $B/forum/$FORUM/new)" 422
check "approval word held" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" --data-urlencode "title=Best casino in town" --data-urlencode "body=Come visit our lovely place" $B/forum/$FORUM/new)" 202
if grep -q "Best casino" <<<"$(curl -s $B/forum/2-introductions/)"; then echo "FAIL held topic is public"; fail=1; else echo "ok   held topic is not public"; fi
check "bob has no review queue" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/review)" 403
REVIEW=$(curl -s -b $J $B/review)
has "queue shows held topic" "$REVIEW" "Awaiting approval"
RID=$(grep -oE 'action="/review/[0-9]+"' <<<"$REVIEW" | grep -oE '[0-9]+' | head -1)
check "approve held topic" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&decision=keep" $B/review/$RID)" 303
has "approved topic is public" "$(curl -s "$B/forum/2-introductions/?sort=newest")" "Best casino in town"

# --- unread
has "unread lists bob's topic for alice" "$(curl -s -b $J $B/unread)" "Best casino in town"
CASINO=$(grep -oE '/topic/[0-9]+-best-casino-in-town/' <<<"$(curl -s $B/latest)" | head -1)
curl -s -o /dev/null -b $J "$B$CASINO"
if grep -q "Best casino" <<<"$(curl -s -b $J $B/unread)"; then echo "FAIL read topic still unread"; fail=1; else echo "ok   reading clears unread"; fi
CID=$(grep -oE '[0-9]+' <<<"$CASINO" | head -1)
check "bob replies by id" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&body=Bumping my own topic now" $B/topic/$CID/reply)" 303
has "new reply shows as 1 new" "$(curl -s -b $J $B/latest)" "1 new"
check "mark all read" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T" $B/read-all)" 303
if grep -q "Best casino" <<<"$(curl -s -b $J $B/unread)"; then echo "FAIL mark all read"; fail=1; else echo "ok   mark all read clears unread"; fi


# --- topic moderation: history, split, move, merge, delete/restore, IP
OFF=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"forum","title":"Offtopic","parent_id":1}' $B/api/v1/admin/nodes | grep -oE '[0-9]+')
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Split source topic" --data-urlencode "body=The original first post text" $B/forum/$FORUM/new)
SRC=$(grep -oE '/topic/[0-9]+' <<<"$LOC" | grep -oE '[0-9]+')
curl -s -o /dev/null -b $K --data "csrf=$T2&body=First reply that will move" $B/topic/$SRC/reply
curl -s -o /dev/null -b $K --data "csrf=$T2&body=Second reply that will move" $B/topic/$SRC/reply
PAGE=$(curl -s -b $J "$B/topic/$SRC-split-source-topic/")
has "staff see poster IP" "$PAGE" "· 127.0.0.1"
if grep -q "· 127.0.0.1" <<<"$(curl -s -b $K "$B/topic/$SRC-split-source-topic/")"; then echo "FAIL members see IPs"; fail=1; else echo "ok   members do not see IPs"; fi
P1=$(grep -oE 'action="/post/[0-9]+/edit"|href="/post/[0-9]+/edit"' <<<"$PAGE" | head -1 | grep -oE '[0-9]+')
check "alice edits first post" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" --data-urlencode "body=The edited first post text" --data-urlencode "reason=typo fix" $B/post/$P1/edit)" 303
HIST=$(curl -s -b $J $B/post/$P1/history)
has "history shows old text" "$HIST" "The original first post text"
has "history shows reason" "$HIST" "typo fix"
REPLIES=$(grep -oE 'name="post" value="[0-9]+"' <<<"$PAGE" | grep -oE '[0-9]+' | tr '\n' ' ')
ARGS=""; for r in $REPLIES; do ARGS="$ARGS --data post=$r"; done
check "bob cannot split" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&title=Stolen split topic" $ARGS $B/topic/$SRC/split)" 403
NEWLOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data "csrf=$T&title=Split result topic" $ARGS $B/topic/$SRC/split)
has "split creates topic" "$NEWLOC" "split-result-topic"
NEW=$(grep -oE '/topic/[0-9]+' <<<"$NEWLOC" | grep -oE '[0-9]+')
has "split topic has moved replies" "$(curl -s "$B/topic/$NEW-split-result-topic/")" "Second reply that will move"
if grep -q "First reply that will move" <<<"$(curl -s -b $J "$B/topic/$SRC-split-source-topic/")"; then echo "FAIL replies still in source"; fail=1; else echo "ok   replies left the source topic"; fi
check "move to offtopic" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&to=$OFF" $B/topic/$NEW/move)" 303
has "offtopic lists moved topic" "$(curl -s -b $J "$B/forum/$OFF-offtopic/")" "Split result topic"
check "merge back into source" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&into=$SRC" $B/topic/$NEW/merge)" 303
check "merged topic URL redirects" "$(curl -s -o /dev/null -w '%{redirect_url}' "$B/topic/$NEW-split-result-topic/")" "$B/topic/$SRC-split-source-topic/"
has "source has merged replies" "$(curl -s -b $J "$B/topic/$SRC-split-source-topic/")" "Second reply that will move"
DEL=$(grep -oE 'action="/post/[0-9]+/delete"' <<<"$(curl -s -b $J "$B/topic/$SRC-split-source-topic/")" | tail -1 | grep -oE '[0-9]+')
check "delete a reply" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T" $B/post/$DEL/delete)" 303
has "deletion log lists it" "$(curl -s -b $J $B/review/deleted)" "action=\"/post/$DEL/restore\""
check "bob cannot restore" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2" $B/post/$DEL/restore)" 403
check "restore" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T" $B/post/$DEL/restore)" 303
if grep -q "action=\"/post/$DEL/restore\"" <<<"$(curl -s -b $J $B/review/deleted)"; then echo "FAIL restored post still in log"; fail=1; else echo "ok   restored post left the log"; fi

# --- profiles, avatars, ignore, follows, notification preferences
has "letter avatar" "$(curl -s $B/avatar/alice.svg)" ">A</text>"
check "avatar rejects bad names" "$(curl -s -o /dev/null -w '%{http_code}' $B/avatar/alice.png)" 404
check "bad website rejected" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" --data-urlencode "website=javascript:alert(1)" $B/settings/profile)" 422
check "save profile" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" --data "location=Krak%C3%B3w" --data-urlencode "website=https://bob.example" --data-urlencode "about=I like **forums**" $B/settings/profile)" 303
BOBP=$(curl -s $B/profile/2-bob/)
has "profile shows location" "$BOBP" "Kraków"
has "profile website is ugc" "$BOBP" 'href="https://bob.example" rel="nofollow ugc noopener"'
has "profile about is cooked" "$BOBP" "<strong>forums</strong>"
check "bob ignores alice" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&on=1" $B/profile/1/ignore)" 303
has "alice's posts collapse for bob" "$(curl -s -b $K "$B/topic/$SRC-split-source-topic/")" "Post by alice, who you ignore"
check "bob stops ignoring alice" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&on=0" $B/profile/1/ignore)" 303
check "cannot ignore self" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&on=1" $B/profile/2/ignore)" 422
check "bob watches introductions" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&level=watch" $B/forum/$FORUM/follow)" 303
has "forum shows watching" "$(curl -s -b $K "$B/forum/$FORUM-introductions/")" '<option value="watch" selected>'
check "bob wants reply emails" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&reply_app=on&reply_email=on&mention_app=on" $B/settings/notifications)" 303
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Watched forum topic" --data-urlencode "body=Hello watchers of this forum" $B/forum/$FORUM/new)
WT=$(grep -oE '/topic/[0-9]+' <<<"$LOC" | grep -oE '[0-9]+')
has "forum watcher notified in app" "$(curl -s -b $K $B/notifications)" "alice posted in Watched forum topic"
if [ -n "$MAIL" ]; then
  for _ in $(seq 20); do grep -ql "Watched forum topic" "$MAIL"/*.eml 2>/dev/null && break; sleep 0.5; done
  has "forum watcher notified by email" "$(grep -l "To: bob@example.com" "$MAIL"/*.eml | xargs cat)" 'alice posted in "Watched forum topic"'
fi
check "bob mutes the topic" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&level=mute" $B/topic/$WT/follow)" 303
curl -s -o /dev/null -b $J --data "csrf=$T&body=A reply bob should not hear about" $B/topic/$WT/reply
N=$(curl -s -b $K $B/notifications | grep -c "alice posted in Watched forum topic")
check "muted topic is quiet" "$N" 1
check "alice auto-watches her topic" "$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/" | grep -c '<option value="watch" selected>')" 1

# --- uploads, avatars, drafts
TMPF=$(mktemp -d)
# A valid 1x1 PNG.
printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==' | base64 -d > $TMPF/dot.png
printf '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>' > $TMPF/x.svg
printf '<html><script>alert(1)</script></html>' > $TMPF/fake.png
printf 'plain notes\n' > $TMPF/notes.txt
U=$(curl -s -b $K -H "X-CSRF-Token: $T2" -F "file=@$TMPF/dot.png" $B/api/v1/uploads)
has "upload image" "$U" '"is_image":true'
has "image dimensions read" "$U" '"width":1'
IMG=$(grep -oE '/uploads/[0-9a-f]{64}\.png' <<<"$U" | head -1)
has "markdown snippet" "$U" "![dot.png]($IMG#"
U2=$(curl -s -b $K -H "X-CSRF-Token: $T2" -F "file=@$TMPF/dot.png" $B/api/v1/uploads)
check "same bytes dedupe" "$(echo "$U2" | grep -oE '/uploads/[0-9a-f]{64}\.png' | head -1)" "$IMG"
H=$(curl -s -D - -o /dev/null $B$IMG)
has "image served as png" "$H" "content-type: image/png"
has "uploads are nosniff" "$H" "x-content-type-options: nosniff"
has "uploads are sandboxed" "$H" "sandbox"
check "svg rejected" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -H "X-CSRF-Token: $T2" -F "file=@$TMPF/x.svg" $B/api/v1/uploads)" 422
check "html disguised as png rejected" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -H "X-CSRF-Token: $T2" -F "file=@$TMPF/fake.png" $B/api/v1/uploads)" 422
check "upload needs csrf" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -F "file=@$TMPF/dot.png" $B/api/v1/uploads)" 400
check "guests cannot upload" "$(curl -s -o /dev/null -w '%{http_code}' -H "X-CSRF-Token: $T2" -F "file=@$TMPF/dot.png" $B/api/v1/uploads)" 403
TXT=$(curl -s -b $K -H "X-CSRF-Token: $T2" -F "file=@$TMPF/notes.txt" $B/api/v1/uploads | grep -oE '/uploads/[0-9a-f]{64}\.txt')
has "files download as attachments" "$(curl -s -D - -o /dev/null $B$TXT)" "content-disposition: attachment; filename*=UTF-8''notes.txt"
check "unknown upload 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/uploads/$(printf 'b%.0s' $(seq 64)).png)" 404
check "path tricks 404" "$(curl -s -o /dev/null -w '%{http_code}' "$B/uploads/..%2f..%2fCargo.toml")" 404
check "upload avatar" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -F "csrf=$T2" -F "file=@$TMPF/dot.png" $B/settings/avatar)" 303
check "avatar url points at upload" "$(curl -s -o /dev/null -w '%{redirect_url}' $B/avatar/bob.svg)" "$B$IMG"
check "text is not an avatar" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -F "csrf=$T2" -F "file=@$TMPF/notes.txt" $B/settings/avatar)" 422
check "remove avatar" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2" $B/settings/avatar/remove)" 303
has "letter avatar again" "$(curl -s $B/avatar/bob.svg)" ">B</text>"
check "save draft" "$(curl -s -o /dev/null -w '%{http_code}' -b $K -X PUT -H "X-CSRF-Token: $T2" -H 'Content-Type: application/json' -d '{"body":"half-written reply"}' "$B/api/v1/drafts?context=reply:$WT")" 204
has "load draft" "$(curl -s -b $K "$B/api/v1/drafts?context=reply:$WT")" "half-written reply"
check "draft is private" "$(curl -s -o /dev/null -w '%{http_code}' -b $J "$B/api/v1/drafts?context=reply:$WT")" 204
check "bad draft context" "$(curl -s -o /dev/null -w '%{http_code}' -b $K "$B/api/v1/drafts?context=../x")" 400
curl -s -o /dev/null -b $K --data "csrf=$T2&body=The finished reply text" $B/topic/$WT/reply
check "posting clears the draft" "$(curl -s -o /dev/null -w '%{http_code}' -b $K "$B/api/v1/drafts?context=reply:$WT")" 204
rm -rf $TMPF

# --- admin control panel API (session admin)
A() { curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' "$@"; }
has "admin stats" "$(A $B/api/v1/admin/stats)" '"users":2'
check "admin write needs csrf" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H 'Content-Type: application/json' -X POST -d '{"name":"x"}' $B/api/v1/admin/groups)" 400
check "members cannot read admin API" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/api/v1/admin/stats)" 403
has "admin user search" "$(A "$B/api/v1/admin/users?q=bo")" '"username":"bob"'
check "create group" "$(A -o /dev/null -w '%{http_code}' -d '{"name":"Helpers"}' $B/api/v1/admin/groups)" 201
check "duplicate group rejected" "$(A -o /dev/null -w '%{http_code}' -d '{"name":"Helpers"}' $B/api/v1/admin/groups)" 422
check "admin cannot demote self" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"primary_group_id":2}' $B/api/v1/admin/users/1)" 422
check "silence bob" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"silenced_until":"2099-01-01T00:00:00Z"}' $B/api/v1/admin/users/2)" 204
check "silenced bob cannot post" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" --data-urlencode "title=Silenced topic title" --data-urlencode "body=Silenced body text here" $B/forum/$FORUM/new)" 403
check "unsilence bob" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"silenced_until":null}' $B/api/v1/admin/users/2)" 204
has "tree lists offtopic" "$(A $B/api/v1/admin/tree)" '"title":"Offtopic"'
check "hide offtopic from guests" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"2":31,"3":63,"4":63}' $B/api/v1/admin/nodes/$OFF/permissions)" 204
check "guests no longer see offtopic" "$(curl -s -o /dev/null -w '%{http_code}' "$B/forum/$OFF-offtopic/")" 404
check "members still see offtopic" "$(curl -s -o /dev/null -w '%{http_code}' -b $K "$B/forum/$OFF-offtopic/")" 200
check "rename forum" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"title":"Off Topic"}' $B/api/v1/admin/nodes/$OFF)" 204
check "renamed forum old slug redirects" "$(curl -s -o /dev/null -w '%{redirect_url}' -b $K "$B/forum/$OFF-offtopic/")" "$B/forum/$OFF-off-topic/"
check "cycle rejected" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d "{\"parent_id\":$OFF}" $B/api/v1/admin/nodes/1)" 422
check "bob has no review queue yet" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/review)" 403
check "make bob a moderator of off topic" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"usernames":["bob"]}' $B/api/v1/admin/nodes/$OFF/moderators)" 204
check "bob now has a review queue" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/review)" 200
check "bob still cannot pin outside his forum" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data "csrf=$T2&action=pin" $B/topic/$SRC/mod)" 403
check "remove bob as moderator" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"usernames":[]}' $B/api/v1/admin/nodes/$OFF/moderators)" 204
check "unknown moderator rejected" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"usernames":["nobody"]}' $B/api/v1/admin/nodes/$OFF/moderators)" 422
has "audit log records actions" "$(A "$B/api/v1/admin/audit?action=node.")" '"action":"node.moderators"'
check "suspend bob" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"suspended_until":"2099-01-01T00:00:00Z"}' $B/api/v1/admin/users/2)" 204
check "suspended bob is signed out" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/notifications)" 403
check "unsuspend bob" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"suspended_until":null}' $B/api/v1/admin/users/2)" 204
has "admin SPA serves client routes" "$(curl -s $B/admin/users/)" "<!doctype html>"

# --- account security (needs BF_MAIL_DIR)
if [ -n "$MAIL" ]; then
  V=$(mail_link alice@example.com /verify/)
  has "verification mail sent" "$V" "/verify/"
  has "verify link works" "$(curl -s "${V/http:\/\/127.0.0.1:8099/$B}")" "Email verified"
  check "verify link is single use" "$(curl -s -o /dev/null -w '%{http_code}' "${V/http:\/\/127.0.0.1:8099/$B}")" 410
  has "settings shows verified" "$(curl -s -b $J $B/settings)" "alice@example.com — verified"

  G=$(mktemp)
  curl -s -c $G -b $G $B/forgot >/dev/null; TG=$(csrf $G)
  has "forgot (unknown address) does not reveal accounts" "$(curl -s -c $G -b $G --data "csrf=$TG&email=nobody@example.com" $B/forgot)" "If an account uses that address"
  has "forgot for bob" "$(curl -s -c $G -b $G --data "csrf=$TG&email=BOB@example.com" $B/forgot)" "If an account uses that address"
  R=$(mail_link bob@example.com /reset/); R=${R/http:\/\/127.0.0.1:8099/$B}
  check "reset form opens" "$(curl -s -o /dev/null -w '%{http_code}' -b $G $R)" 200
  check "weak password rejected" "$(curl -s -o /dev/null -w '%{http_code}' -c $G -b $G --data "csrf=$TG&password=short" $R)" 422
  check "reset password" "$(curl -s -o /dev/null -w '%{http_code}' -c $G -b $G --data "csrf=$TG&password=brand new passphrase" $R)" 303
  check "reset link is single use" "$(curl -s -o /dev/null -w '%{http_code}' -b $G $R)" 410
  has "reset signs bob in" "$(curl -s -b $G $B/settings)" "bob@example.com"
  check "bob's old session was revoked" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/notifications)" 403
  L=$(mktemp); curl -s -c $L -b $L $B/login >/dev/null; TL=$(csrf $L)
  check "old password fails" "$(curl -s -o /dev/null -w '%{http_code}' -c $L -b $L --data "csrf=$TL&login=bob&password=another long passphrase" $B/login)" 422
  check "new password works" "$(curl -s -o /dev/null -w '%{http_code}' -c $L -b $L --data "csrf=$TL&login=bob&password=brand new passphrase" $B/login)" 303
  check "wrong current password" "$(curl -s -o /dev/null -w '%{http_code}' -b $L --data "csrf=$TL&current=nope-nope-nope&new=yet another passphrase" $B/settings/password)" 422
  S=$(curl -s -b $L $B/settings)
  has "two sessions listed" "$(grep -c 'This device\|Signed in' <<<"$S")" "2"
  OTHER=$(grep -oE 'name="id" value="[0-9a-f]{64}"' <<<"$S" | head -1 | grep -oE '[0-9a-f]{64}')
  check "revoke other session" "$(curl -s -o /dev/null -w '%{http_code}' -b $L --data "csrf=$TL&id=$OTHER" $B/settings/sessions/revoke)" 303
  check "revoked session is a guest" "$(curl -s -o /dev/null -w '%{http_code}' -b $G $B/notifications)" 403

  SETUP=$(curl -s -b $L "$B/settings?setup=2fa")
  SECRET=$(grep -oE 'name="secret" value="[A-Z2-7]+"' <<<"$SETUP" | grep -oE '[A-Z2-7]{20,}')
  check "wrong code does not enable 2fa" "$(curl -s -o /dev/null -w '%{http_code}' -b $L --data "csrf=$TL&secret=$SECRET&code=000000" $B/settings/2fa/enable)" 422
  has "enable 2fa" "$(curl -s -b $L --data "csrf=$TL&secret=$SECRET&code=$(totp $SECRET 0)" $B/settings/2fa/enable)" "Two-factor authentication is on"
  M=$(mktemp); curl -s -c $M -b $M $B/login >/dev/null; TM=$(csrf $M)
  check "password step redirects to 2fa" "$(curl -s -o /dev/null -w '%{redirect_url}' -c $M -b $M --data "csrf=$TM&login=bob&password=brand new passphrase" $B/login)" "$B/login/2fa"
  check "not signed in before code" "$(curl -s -o /dev/null -w '%{http_code}' -b $M $B/notifications)" 403
  check "wrong code" "$(curl -s -o /dev/null -w '%{http_code}' -c $M -b $M --data "csrf=$TM&code=123456" $B/login/2fa)" 422
  # The enable step used the current 30 s step; the next step is accepted (±1 skew).
  check "correct code signs in" "$(curl -s -o /dev/null -w '%{http_code}' -c $M -b $M --data "csrf=$TM&code=$(totp $SECRET 30)" $B/login/2fa)" 303
  check "signed in after code" "$(curl -s -o /dev/null -w '%{http_code}' -b $M $B/notifications)" 200
  rm -f $G $L $M
else
  echo "skip account security checks (set BF_MAIL_DIR)"
fi

# --- likes, top, unlisted, edit grace, legal, age gate, account approval
LIKE=$(grep -oE 'action="/post/[0-9]+/like"' <<<"$(curl -s -b $J "$B/topic/$SRC-split-source-topic/")" | head -1 | grep -oE '[0-9]+')
check "like a reply" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T" $B/post/$LIKE/like)" 303
has "like count shows" "$(curl -s -b $J "$B/topic/$SRC-split-source-topic/")" 'aria-pressed="true">♥ 1'
UL=$(curl -s -b $J -H 'Accept: application/json' --data "csrf=$T" $B/post/$LIKE/like)
has "unlike via json" "$UL" '"liked":false'
has "unlike count" "$UL" '"count":0'
OWN=$(grep -oE 'href="/post/[0-9]+/edit"' <<<"$(curl -s -b $J "$B/topic/$SRC-split-source-topic/")" | head -1 | grep -oE '[0-9]+')
check "cannot like own post" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T" $B/post/$OWN/like)" 422
has "top all-time lists topics" "$(curl -s "$B/top?period=all")" "Hello from Alice"
has "top has period links" "$(curl -s "$B/top?period=week")" 'class="current" aria-current="true">week'
check "unlist topic" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&action=unlist" $B/topic/$WT/mod)" 303
if grep -q "Watched forum topic" <<<"$(curl -s $B/latest)"; then echo "FAIL unlisted topic in latest"; fail=1; else echo "ok   unlisted topic left latest"; fi
has "unlisted topic is noindex" "$(curl -s "$B/topic/$WT-watched-forum-topic/")" 'name="robots" content="noindex"'
if grep -q "/topic/$WT-" <<<"$(curl -s $B/sitemap.xml)"; then echo "FAIL unlisted topic in sitemap"; fail=1; else echo "ok   unlisted topic left the sitemap"; fi
check "relist topic" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&action=list" $B/topic/$WT/mod)" 303
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data "csrf=$T&body=Quick reply with a typo" $B/topic/$WT/reply)
QN=$(grep -oE '#post-[0-9]+' <<<"$LOC" | grep -oE '[0-9]+')
QID=$(grep -oE "id=\"post-$QN\"" -A3 <<<"$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/")" >/dev/null; grep -oE 'href="/post/[0-9]+/edit"' <<<"$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/")" | tail -1 | grep -oE '[0-9]+')
check "quick fix inside grace period" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Quick reply without a typo" $B/post/$QID/edit)" 303
if grep -q "<h2>Edit " <<<"$(curl -s -b $J $B/post/$QID/history)"; then echo "FAIL grace edit kept a revision"; fail=1; else echo "ok   grace edit kept no revision"; fi
check "second edit is recorded" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Quick reply, edited again&reason=more" $B/post/$QID/edit)" 303
has "second edit has a revision" "$(curl -s -b $J $B/post/$QID/history)" "<h2>Edit 1</h2>"

check "publish terms" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"body":"Be **kind**."}' $B/api/v1/admin/legal/terms)" 201
check "publish privacy" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"body":"We keep little data."}' $B/api/v1/admin/legal/privacy)" 201
has "terms page" "$(curl -s $B/legal/terms)" "<strong>kind</strong>"
check "existing member must accept before posting" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Posting before accepting" $B/topic/$WT/reply)" 422
DOCS=$(grep -oE 'name="docs" value="[0-9,]+"' <<<"$(curl -s -b $J $B/legal/accept)" | grep -oE '[0-9,]+$|[0-9]+,[0-9]+')
check "accept terms" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&docs=$DOCS" $B/legal/accept)" 303
check "posting works after accepting" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Posting after accepting" $B/topic/$WT/reply)" 303

check "set minimum age" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"value":16}' $B/api/v1/admin/settings/minimum_age)" 204
check "require approval" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"value":true}' $B/api/v1/admin/settings/require_account_approval)" 204
C=$(mktemp); curl -s -c $C -b $C $B/register >/dev/null; TC=$(csrf $C)
REG="csrf=$TC&username=carol&email=carol@example.com&password=carol long passphrase"
has "register shows terms" "$(curl -s -b $C $B/register)" 'href="/legal/terms"'
check "must accept terms" "$(curl -s -o /dev/null -w '%{http_code}' -c $C -b $C --data "$REG&birth_date=1990-01-01" $B/register)" 422
check "too young" "$(curl -s -o /dev/null -w '%{http_code}' -c $C -b $C --data "$REG&birth_date=$(date +%Y)-01-01&accept=1" $B/register)" 422
check "carol registers" "$(curl -s -o /dev/null -w '%{http_code}' -c $C -b $C --data "$REG&birth_date=1990-01-01&accept=1" $B/register)" 303
check "unapproved carol cannot post" "$(curl -s -o /dev/null -w '%{http_code}' -b $C --data-urlencode "csrf=$TC" --data-urlencode "title=Carol first topic here" --data-urlencode "body=Hello everyone from Carol" $B/forum/$FORUM/new)" 422
has "admin sees carol awaiting approval" "$(A "$B/api/v1/admin/users?q=carol")" '"approved":false'
CID=$(A "$B/api/v1/admin/users?q=carol" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "approve carol" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"approved":true}' $B/api/v1/admin/users/$CID)" 204
check "approved carol can post" "$(curl -s -o /dev/null -w '%{http_code}' -b $C --data-urlencode "csrf=$TC" --data-urlencode "title=Carol first topic here" --data-urlencode "body=Hello everyone from Carol" $B/forum/$FORUM/new)" 303
A -o /dev/null -X PUT -d '{"value":0}' $B/api/v1/admin/settings/minimum_age
A -o /dev/null -X PUT -d '{"value":false}' $B/api/v1/admin/settings/require_account_approval
rm -f $C

# --- search filters, live search API, polling state, hide, link forums
has "search by author" "$(curl -s -b $J "$B/search?q=topic&author=alice")" "Watched forum topic"
if grep -q "Carol first topic" <<<"$(curl -s -b $J "$B/search?q=topic&author=alice")"; then echo "FAIL author filter leaks"; fail=1; else echo "ok   author filter excludes others"; fi
has "search titles only" "$(curl -s -b $J "$B/search?q=watched&in=topics")" "Watched forum topic"
if grep -q "<a href=\"/topic" <<<"$(curl -s -b $J "$B/search?q=watched&after=2999-01-01")"; then echo "FAIL date filter"; fail=1; else echo "ok   date filter excludes older posts"; fi
has "search in one forum" "$(curl -s -b $J "$B/search?q=reply&forum=$FORUM")" "<ol class=\"results\">"
has "search api returns json" "$(curl -s "$B/api/v1/search?q=watched")" '"title":"Watched forum topic"'
has "topic state endpoint" "$(curl -s "$B/api/v1/topics/$WT/state")" '"highest_post_number":'
HID=$(grep -oE 'action="/post/[0-9]+/hide"' <<<"$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/")" | tail -1 | grep -oE '[0-9]+')
check "hide a post" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&hidden=1&reason=off topic" $B/post/$HID/hide)" 303
has "moderators see why it is hidden" "$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/")" "Hidden from members: off topic"
if grep -q "Hidden from members" <<<"$(curl -s "$B/topic/$WT-watched-forum-topic/")"; then echo "FAIL guests see hidden post"; fail=1; else echo "ok   guests do not see the hidden post"; fi
check "unhide" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&hidden=0" $B/post/$HID/hide)" 303
check "link forum needs url" "$(A -o /dev/null -w '%{http_code}' -d '{"kind":"link","title":"Docs","parent_id":1}' $B/api/v1/admin/nodes)" 422
check "create link forum" "$(A -o /dev/null -w '%{http_code}' -d '{"kind":"link","title":"Docs","parent_id":1,"url":"https://docs.example.com/"}' $B/api/v1/admin/nodes)" 201
has "link forum on index" "$(curl -s $B/)" '/go"'

# --- cooking: highlighting, oneboxes; quote source; member bundle
BODY=$(printf 'Code and video:\n\n```rust\nfn main() { let x = 1; }\n```\n\nhttps://www.youtube.com/watch?v=dQw4w9WgXcQ\n')
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode "body=$BODY" $B/topic/$WT/reply
TP=$(curl -s "$B/topic/$WT-watched-forum-topic/")
has "code is highlighted" "$TP" 'class="hl-'
has "youtube onebox" "$TP" 'src="https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"'
if grep -q "<iframe" <<<"$TP"; then echo "FAIL iframe in page"; fail=1; else echo "ok   no iframes"; fi
QP=$(grep -oE 'data-quote="[0-9]+"' <<<"$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/")" | head -1 | grep -oE '[0-9]+')
has "raw source for quoting" "$(curl -s -b $J $B/api/v1/posts/$QP/raw)" '"raw":"Hello watchers of this forum"'
check "raw source respects permissions" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/v1/posts/999999/raw)" 404
MJS=$(grep -oE '/assets/members\.[0-9a-f]+\.js' <<<"$TP" | head -1)
check "member bundle served" "$(curl -s -o /dev/null -w '%{http_code}' $B$MJS)" 200

# --- OAuth / OIDC (needs tests/support/mock-oidc.mjs on $OIDC and the logins set up as site settings: the vitest suite in tests/e2e covers these flows; set BF_E2E_LOGINS=1 once they are)
# These sign-ins start with a GET, which `auth_require_interaction` (on by default) turns into a page with a button.
A -o /dev/null -X PUT -d '{"value":false}' $B/api/v1/admin/settings/auth_require_interaction
if [ -n "${OIDC:-}" ] && [ -n "${BF_E2E_LOGINS:-}" ]; then
  next_user() { curl -s -o /dev/null -X POST -d "$1" $OIDC/next-user; }
  has "login page offers sso" "$(curl -s $B/login)" 'action="/auth/oidc"'
  D=$(mktemp)
  next_user '{"sub":"ext-1","email":"dave@example.com","email_verified":true,"preferred_username":"dave"}'
  check "sso creates account" "$(curl -s -L -o /dev/null -w '%{url_effective}' -c $D -b $D $B/auth/oidc)" "$B/"
  has "sso user signed in" "$(curl -s -b $D $B/settings)" "dave@example.com"
  rm -f $D; D=$(mktemp)
  check "second sso login" "$(curl -s -L -o /dev/null -w '%{http_code}' -c $D -b $D $B/auth/oidc)" 200
  has "same account, no duplicate" "$(curl -s -b $D $B/)" '<strong>dave</strong></div>'
  rm -f $D; D=$(mktemp)
  next_user '{"sub":"ext-2","email":"someone-else@example.com","email_verified":true,"preferred_username":"whoever"}'
  check "signed-in member links sso" "$(curl -s -L -o /dev/null -w '%{url_effective}' -c $J -b $J $B/auth/oidc)" "$B/settings"
  curl -s -L -o /dev/null -c $D -b $D $B/auth/oidc
  has "linked sso signs into alice" "$(curl -s -b $D $B/)" '<strong>alice</strong></div>'
  rm -f $D; D=$(mktemp)
  next_user '{"sub":"ext-3","email":"carol@example.com","email_verified":false,"preferred_username":"carol"}'
  check "unverified email cannot take over carol" "$(curl -s -L -o /dev/null -w '%{http_code}' -c $D -b $D $B/auth/oidc)" 422
  check "and nobody is signed in" "$(curl -s -o /dev/null -w '%{http_code}' -b $D $B/notifications)" 403
  rm -f $D; D=$(mktemp)
  curl -s -o /dev/null -c $D -b $D $B/auth/oidc
  check "forged state rejected" "$(curl -s -o /dev/null -w '%{http_code}' -b $D "$B/auth/oidc/callback?code=x&state=forged")" 400
  check "missing state cookie rejected" "$(curl -s -o /dev/null -w '%{http_code}' "$B/auth/oidc/callback?code=x&state=y")" 400
  check "unknown provider 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/auth/myspace)" 404
  rm -f $D
else
  echo "skip sso checks (set OIDC)"
fi

# --- custom profile fields, registration questions, group limits, prefixes
check "select field needs options" "$(A -o /dev/null -w '%{http_code}' -d '{"label":"Colour","kind":"select","options":["Red"]}' $B/api/v1/admin/profile-fields)" 422
check "create register question" "$(A -o /dev/null -w '%{http_code}' -d '{"label":"Favourite language","kind":"select","options":["Rust","Go","Other"],"required":true,"show_on_register":true}' $B/api/v1/admin/profile-fields)" 201
FLD=$(A $B/api/v1/admin/profile-fields | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
E=$(mktemp); curl -s -c $E -b $E $B/register >/dev/null; TE=$(csrf $E)
has "register asks the question" "$(curl -s -b $E $B/register)" "Favourite language"
REGE="csrf=$TE&username=erin&email=erin@example.com&password=erin long passphrase&accept=1"
check "required question enforced" "$(curl -s -o /dev/null -w '%{http_code}' -c $E -b $E --data "$REGE" $B/register)" 422
check "bad option rejected" "$(curl -s -o /dev/null -w '%{http_code}' -c $E -b $E --data "$REGE&field_$FLD=Cobol" $B/register)" 422
check "erin registers" "$(curl -s -o /dev/null -w '%{http_code}' -c $E -b $E --data "$REGE&field_$FLD=Rust" $B/register)" 303
EID=$(A "$B/api/v1/admin/users?q=erin" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
has "answer on profile" "$(curl -s $B/profile/$EID-erin/)" "<dt>Favourite language</dt><dd>Rust</dd>"
has "answer editable in settings" "$(curl -s -b $E $B/settings)" "<option selected>Rust</option>"
check "limit members to 0 posts a day" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"posts_per_day":0}' $B/api/v1/admin/groups/2/limits)" 204
check "unknown limit rejected" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"nope":1}' $B/api/v1/admin/groups/2/limits)" 422
check "limited member cannot post" "$(curl -s -o /dev/null -w '%{http_code}' -b $E --data "csrf=$TE&body=Erin says hello there" $B/topic/$WT/reply)" 422
check "admins are not limited" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Admin reply under limits" $B/topic/$WT/reply)" 303
check "clear limits" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{}' $B/api/v1/admin/groups/2/limits)" 204
check "member can post again" "$(curl -s -o /dev/null -w '%{http_code}' -b $E --data "csrf=$TE&body=Erin says hello there" $B/topic/$WT/reply)" 303
check "set forum prefixes" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '["Question","Solved"]' $B/api/v1/admin/nodes/$FORUM/prefixes)" 204
has "composer offers prefixes" "$(curl -s -b $J $B/forum/$FORUM/new)" "<option>Question</option>"
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "prefix=Question" --data-urlencode "title=How do prefixes work" --data-urlencode "body=Asking about prefixes here" $B/forum/$FORUM/new)
has "topic shows prefix" "$(curl -s "$LOC")" '<span class="prefix">Question</span>'
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode "prefix=Bogus" --data-urlencode "title=Unknown prefix topic" --data-urlencode "body=Prefix that is not allowed" $B/forum/$FORUM/new
PF=$(curl -s "$B/forum/$FORUM-introductions/?prefix=Question")
has "prefix filter shows matching" "$PF" "How do prefixes work"
if grep -q "Unknown prefix topic" <<<"$PF"; then echo "FAIL prefix filter"; fail=1; else echo "ok   prefix filter hides others"; fi
if grep -q '<span class="prefix">Bogus' <<<"$(curl -s "$B/forum/$FORUM-introductions/")"; then echo "FAIL unknown prefix stored"; fail=1; else echo "ok   unknown prefixes are ignored"; fi
check "digest preference saved" "$(curl -s -o /dev/null -w '%{http_code}' -b $E --data "csrf=$TE&digest=weekly&reply_app=on" $B/settings/notifications)" 303
has "digest preference shown" "$(curl -s -b $E $B/settings)" '<option value="weekly" selected>'
rm -f $E

# --- passkeys (driven by tests/support/soft-passkey.mjs, a software authenticator)
PK=$(mktemp -d); SOFT="node $(dirname "$0")/../support/soft-passkey.mjs"
OPTS=$(curl -s -c $J -b $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/passkeys/register/options)
has "register options" "$OPTS" '"alg":-7'
REGB=$($SOFT register $PK/k.json $B "$OPTS")
check "register passkey" "$(curl -s -o /dev/null -w '%{http_code}' -c $J -b $J -X POST -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d "$REGB" $B/api/v1/passkeys/register)" 201
check "challenge is single use" "$(curl -s -o /dev/null -w '%{http_code}' -c $J -b $J -X POST -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d "$REGB" $B/api/v1/passkeys/register)" 400
has "settings lists passkey" "$(curl -s -b $J $B/settings)" "Soft key"
P=$(mktemp)
LOPTS=$(curl -s -c $P -b $P -X POST $B/api/v1/passkeys/login/options)
LB=$($SOFT login $PK/k.json $B "$LOPTS")
check "sign in with passkey" "$(curl -s -o /dev/null -w '%{http_code}' -c $P -b $P -X POST -H 'Content-Type: application/json' -d "$LB" $B/api/v1/passkeys/login)" 303
has "passkey session is alice" "$(curl -s -b $P $B/)" '<strong>alice</strong></div>'
rm -f $P; P=$(mktemp)
curl -s -o /dev/null -c $P -b $P -X POST $B/api/v1/passkeys/login/options
check "replayed assertion rejected" "$(curl -s -o /dev/null -w '%{http_code}' -c $P -b $P -X POST -H 'Content-Type: application/json' -d "$LB" $B/api/v1/passkeys/login)" 422
LOPTS=$(curl -s -c $P -b $P -X POST $B/api/v1/passkeys/login/options)
BAD=$($SOFT login $PK/k.json https://evil.example "$LOPTS")
check "wrong origin rejected" "$(curl -s -o /dev/null -w '%{http_code}' -c $P -b $P -X POST -H 'Content-Type: application/json' -d "$BAD" $B/api/v1/passkeys/login)" 422
check "no challenge cookie rejected" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d "$LB" $B/api/v1/passkeys/login)" 400
check "guests cannot register" "$(curl -s -o /dev/null -w '%{http_code}' -X POST $B/api/v1/passkeys/register/options)" 400
PKID=$(grep -oE 'action="/settings/passkeys/[A-Za-z0-9_-]+/delete"' <<<"$(curl -s -b $J $B/settings)" | head -1 | sed -E 's#.*/passkeys/([^/]+)/delete.*#\1#')
check "remove passkey" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T" $B/settings/passkeys/$PKID/delete)" 303
rm -f $P; P=$(mktemp)
LOPTS=$(curl -s -c $P -b $P -X POST $B/api/v1/passkeys/login/options)
LB=$($SOFT login $PK/k.json $B "$LOPTS")
check "removed passkey no longer works" "$(curl -s -o /dev/null -w '%{http_code}' -c $P -b $P -X POST -H 'Content-Type: application/json' -d "$LB" $B/api/v1/passkeys/login)" 422
rm -rf $PK $P

# --- email templates and mail log
check "edit verification subject" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"value":"Hi {username}, welcome to {site}"}' $B/api/v1/admin/settings/email_verify_subject)" 204
F=$(mktemp); curl -s -c $F -b $F $B/register >/dev/null; TF=$(csrf $F)
curl -s -o /dev/null -c $F -b $F --data "csrf=$TF&username=frank&email=frank@example.com&password=frank long passphrase&accept=1&field_$FLD=Go" $B/register
if [ -n "$MAIL" ]; then
  for _ in $(seq 20); do grep -ql "frank@example.com" "$MAIL"/*.eml 2>/dev/null && break; sleep 0.5; done
  has "edited template used" "$(grep -l "To: frank@example.com" "$MAIL"/*.eml | xargs cat)" "Subject: Hi frank, welcome to Boring Forum"
  has "link still included" "$(grep -l "To: frank@example.com" "$MAIL"/*.eml | xargs cat)" "/verify/"
fi
has "mail log lists messages" "$(A $B/api/v1/admin/mail)" '"to_addr":"frank@example.com"'
check "mail log is admin only" "$(curl -s -o /dev/null -w '%{http_code}' -b $F $B/api/v1/admin/mail)" 403
A -o /dev/null -X PUT -d '{"value":"Confirm your email for {site}"}' $B/api/v1/admin/settings/email_verify_subject
rm -f $F

# --- account hardening: honeypot, time-to-submit, lockout, backup codes, forced 2FA, email and username changes
H=$(mktemp); curl -s -c $H -b $H $B/register >/dev/null; TH=$(csrf $H)
REGH="csrf=$TH&username=bot1&email=bot1@example.com&password=bot long passphrase&accept=1&field_$FLD=Go"
check "honeypot rejects bots" "$(curl -s -o /dev/null -w '%{http_code}' -c $H -b $H --data "$REGH&website_url=http://spam" $B/register)" 422
A -o /dev/null -X PUT -d '{"value":30}' $B/api/v1/admin/settings/min_register_seconds
TSNOW=$(grep -oE 'name="ts" value="[0-9]+"' <<<"$(curl -s -b $H $B/register)" | grep -oE '[0-9]+')
check "too-fast sign-up rejected" "$(curl -s -o /dev/null -w '%{http_code}' -c $H -b $H --data "$REGH&ts=$TSNOW" $B/register)" 422
check "patient sign-up accepted" "$(curl -s -o /dev/null -w '%{http_code}' -c $H -b $H --data "$REGH&ts=$((TSNOW - 60))" $B/register)" 303
A -o /dev/null -X PUT -d '{"value":0}' $B/api/v1/admin/settings/min_register_seconds
rm -f $H

H=$(mktemp); curl -s -c $H -b $H $B/login >/dev/null; TH=$(csrf $H)
for i in 1 2 3 4 5; do curl -s -o /dev/null -c $H -b $H --data "csrf=$TH&login=carol&password=wrong password $i" $B/login; done
has "account locked after 5 failures" "$(curl -s -c $H -b $H --data "csrf=$TH&login=carol&password=carol long passphrase" $B/login)" "Too many failed attempts"
has "failed logins logged" "$(A $B/api/v1/admin/failed-logins)" '"login":"carol"'
if [ -n "$MAIL" ]; then
  for _ in $(seq 20); do grep -ql "Sign-in attempts" "$MAIL"/*.eml 2>/dev/null && break; sleep 0.5; done
  has "lockout notice emailed" "$(grep -l "Sign-in attempts" "$MAIL"/*.eml | xargs cat)" "To: carol@example.com"
fi
rm -f $H

check "admin cannot force 2fa on own group without it" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"on":true}' $B/api/v1/admin/groups/4/require-2fa)" 422
SETUP=$(curl -s -b $J "$B/settings?setup=2fa")
ASEC=$(grep -oE 'name="secret" value="[A-Z2-7]+"' <<<"$SETUP" | grep -oE '[A-Z2-7]{20,}')
has "alice enables 2fa" "$(curl -s -b $J --data "csrf=$T&secret=$ASEC&code=$(totp $ASEC 0)" $B/settings/2fa/enable)" "Two-factor authentication is on"
CODES=$(curl -s -b $J --data "csrf=$T" $B/settings/2fa/backup-codes)
CODE=$(grep -oE '[0-9a-f]{4}-[0-9a-f]{4}' <<<"$CODES" | head -1)
has "backup codes shown once" "$CODES" "backup codes"
H=$(mktemp); curl -s -c $H -b $H $B/login >/dev/null; TH=$(csrf $H)
curl -s -o /dev/null -c $H -b $H --data "csrf=$TH&login=alice&password=correct horse battery" $B/login
check "backup code signs in" "$(curl -s -o /dev/null -w '%{http_code}' -c $H -b $H --data "csrf=$TH&code=$CODE" $B/login/2fa)" 303
rm -f $H; H=$(mktemp); curl -s -c $H -b $H $B/login >/dev/null; TH=$(csrf $H)
curl -s -o /dev/null -c $H -b $H --data "csrf=$TH&login=alice&password=correct horse battery" $B/login
check "backup code works only once" "$(curl -s -o /dev/null -w '%{http_code}' -c $H -b $H --data "csrf=$TH&code=$CODE" $B/login/2fa)" 422
rm -f $H
check "require 2fa for members" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"on":true}' $B/api/v1/admin/groups/2/require-2fa)" 204
F=$(mktemp); curl -s -c $F -b $F $B/login >/dev/null; TF=$(csrf $F)
curl -s -o /dev/null -c $F -b $F --data "csrf=$TF&login=frank&password=frank long passphrase" $B/login
check "member without 2fa cannot post" "$(curl -s -o /dev/null -w '%{http_code}' -b $F --data "csrf=$TF&body=Frank tries to post here" $B/topic/$WT/reply)" 422
check "stop requiring 2fa" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"on":false}' $B/api/v1/admin/groups/2/require-2fa)" 204
check "member can post again" "$(curl -s -o /dev/null -w '%{http_code}' -b $F --data "csrf=$TF&body=Frank tries to post here" $B/topic/$WT/reply)" 303
rm -f $F

check "email change needs password" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&email=alice2@example.com&password=nope" $B/settings/email)" 422
has "email change sends confirmation" "$(curl -s -b $J --data "csrf=$T&email=alice2@example.com&password=correct horse battery" $B/settings/email)" "Check alice2@example.com"
if [ -n "$MAIL" ]; then
  CE=$(mail_link alice2@example.com /confirm-email/)
  has "confirm new email" "$(curl -s "${CE/http:\/\/127.0.0.1:8099/$B}")" "Your account now uses alice2@example.com"
  has "settings show new email" "$(curl -s -b $J $B/settings)" "alice2@example.com"
fi
check "rename alice" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&username=alicia" $B/settings/username)" 303
check "old mention link redirects" "$(curl -s -o /dev/null -w '%{redirect_url}' $B/u/alice)" "$B/profile/1-alicia/"
check "taken name refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&username=bob" $B/settings/username)" 422
check "second rename too soon" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&username=alice" $B/settings/username)" 422

# --- moderation tools: report reasons, auto-hide, removal message, bulk, notes, delete spammer
newuser() { # $1 name -> prints "jar token"
  local jar; jar=$(mktemp); curl -s -c $jar -b $jar $B/register >/dev/null; local tok; tok=$(csrf $jar)
  curl -s -o /dev/null -c $jar -b $jar --data "csrf=$tok&username=$1&email=$1@example.com&password=$1 long passphrase&accept=1&field_$FLD=Go" $B/register
  echo "$jar $tok"
}
read SJ ST <<<"$(newuser spammy)"
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $SJ --data "csrf=$ST&body=Buy cheap watches now" $B/topic/$WT/reply)
SPN=$(grep -oE '#post-[0-9]+' <<<"$LOC" | grep -oE '[0-9]+')
SPID=$(A "$B/api/v1/admin/users?q=spammy" >/dev/null; grep -oE "id=\"post-$SPN\"" <<<"" ; grep -oE 'data-quote="[0-9]+"' <<<"$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/")" | tail -1 | grep -oE '[0-9]+')
read R1 RT1 <<<"$(newuser rep1)"; read R2 RT2 <<<"$(newuser rep2)"; read R3 RT3 <<<"$(newuser rep3)"
check "other needs a note" "$(curl -s -o /dev/null -w '%{http_code}' -b $R1 --data "csrf=$RT1&reason=other" $B/post/$SPID/report)" 422
curl -s -o /dev/null -b $R1 --data "csrf=$RT1&reason=spam" $B/post/$SPID/report
curl -s -o /dev/null -b $R1 --data "csrf=$RT1&reason=spam" $B/post/$SPID/report
has "one member's repeats do not hide" "$(curl -s "$B/topic/$WT-watched-forum-topic/")" "Buy cheap watches now"
curl -s -o /dev/null -b $R2 --data "csrf=$RT2&reason=spam" $B/post/$SPID/report
curl -s -o /dev/null -b $R3 --data "csrf=$RT3&reason=other&note=advertising" $B/post/$SPID/report
if grep -q "Buy cheap watches now" <<<"$(curl -s "$B/topic/$WT-watched-forum-topic/")"; then echo "FAIL reported post still public"; fail=1; else echo "ok   three reports hide the post"; fi
RQ=$(curl -s -b $J $B/review)
has "queue shows reasons" "$RQ" "Reasons: other, spam"
RID=$(grep -oE 'action="/review/[0-9]+"' <<<"$RQ" | head -1 | grep -oE '[0-9]+')
check "remove with message" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&decision=remove&message=No advertising please" $B/review/$RID)" 303
has "author told why" "$(curl -s -b $SJ $B/notifications)" "No advertising please"
TOPS=$(grep -oE 'name="topic" value="[0-9]+"' <<<"$(curl -s -b $J "$B/forum/$FORUM-introductions/")" | head -2 | grep -oE '[0-9]+')
BARGS=""; for t in $TOPS; do BARGS="$BARGS --data topic=$t"; done
check "bulk lock" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&action=lock" $BARGS $B/forum/$FORUM/bulk)" 303
check "bulk needs a moderator" "$(curl -s -o /dev/null -w '%{http_code}' -b $R1 --data "csrf=$RT1&action=lock" $BARGS $B/forum/$FORUM/bulk)" 403
LOCKED=$(curl -s "$B/forum/$FORUM-introductions/" | grep -c '<span class="tag">Locked</span>')
[ "$LOCKED" -ge 2 ] && echo "ok   bulk-locked topics show as locked" || { echo "FAIL bulk lock ($LOCKED)"; fail=1; }
SPUID=$(A "$B/api/v1/admin/users?q=spammy" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "staff add a note" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Posted ads twice" $B/profile/$SPUID/notes)" 303
has "staff see notes" "$(curl -s -b $J $B/profile/$SPUID-spammy/)" "Posted ads twice"
if grep -q "Posted ads twice" <<<"$(curl -s -b $R1 $B/profile/$SPUID-spammy/)"; then echo "FAIL members see staff notes"; fail=1; else echo "ok   members do not see staff notes"; fi
check "members cannot add notes" "$(curl -s -o /dev/null -w '%{http_code}' -b $R1 --data "csrf=$RT1&body=x" $B/profile/$SPUID/notes)" 403
curl -s -o /dev/null -b $SJ --data-urlencode "csrf=$ST" --data-urlencode "title=Spam topic number two" --data-urlencode "body=More cheap watches here" $B/forum/$FORUM/new
has "delete spammer" "$(A -X POST $B/api/v1/admin/users/$SPUID/delete-spammer)" '"topics_touched":'
if grep -q "Spam topic number two" <<<"$(curl -s "$B/forum/$FORUM-introductions/")"; then echo "FAIL spam topic still listed"; fail=1; else echo "ok   spammer's topics are gone"; fi
check "spammer is signed out" "$(curl -s -o /dev/null -w '%{http_code}' -b $SJ $B/notifications)" 403
rm -f $SJ $R1 $R2 $R3

# --- topic navigation: jumps, short links, posters, sorts, reply-to, pages, suggestions
A -o /dev/null -X PUT -d '{"value":5}' $B/api/v1/admin/settings/posts_per_page
check "jump to post 7" "$(curl -s -o /dev/null -w '%{redirect_url}' "$B/topic/$WT-x/post/7")" "$B/topic/$WT-watched-forum-topic/page/2/#post-7"
check "jump to first unread (guest)" "$(curl -s -o /dev/null -w '%{redirect_url}' "$B/topic/$WT-x/unread")" "$B/topic/$WT-watched-forum-topic/#post-1"
P2=$(curl -s -b $J "$B/topic/$WT-watched-forum-topic/page/2/")
has "numbered page links" "$P2" '<span aria-current="page" class="current">2</span>'
SID=$(grep -oE 'href="/p/[0-9]+"' <<<"$P2" | head -1 | grep -oE '[0-9]+')
SL=$(curl -s -o /dev/null -w "%{redirect_url}" $B/p/$SID)
has "short link redirects to the post" "$SL" "/topic/$WT-watched-forum-topic/page/2/#post-"
LAST=$(grep -oE 'page/[0-9]+/' <<<"$P2" | sort -t/ -k2 -n | tail -1)
has "suggested topics on the last page" "$(curl -s "$B/topic/$WT-watched-forum-topic/$LAST")" "Suggested topics"
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data "csrf=$T&body=Answering post number two&reply_to=2" $B/topic/$WT/reply)
has "in reply to shown" "$(curl -s "$LOC")" "in reply to"
A -o /dev/null -X PUT -d '{"value":20}' $B/api/v1/admin/settings/posts_per_page
has "posters column" "$(curl -s $B/latest)" 'class="hide-sm posters"><img class="avatar"'
S1=$(mktemp)
TT=$(curl -s -c $S1 -b $S1 "$B/forum/$FORUM-introductions/?sort=title")
FIRST=$(grep -oE '<a href="/topic/[0-9]+-[a-z0-9-]+/">[^<]+' <<<"$TT" | head -1 | sed 's/.*">//')
has "title sort orders alphabetically" "$FIRST" "Best casino in town"
has "sort is remembered" "$(curl -s -b $S1 "$B/forum/$FORUM-introductions/")" 'aria-current="true" class="current">Title'
has "most-liked sort exists" "$TT" "sort=likes"
rm -f $S1

# --- index stats, shortcuts, maintenance, offline mode, settings export/import, permission check
HOME=$(curl -s $B/)
has "index statistics" "$HOME" 'aria-label="Statistics"'
has "newest member shown" "$HOME" "Newest member:"
has "shortcuts island present" "$HOME" 'data-island="shortcuts"'
has "recount" "$(A -X POST $B/api/v1/admin/maintenance/recount)" '"recounted":true'
has "recook queued" "$(A -X POST $B/api/v1/admin/maintenance/recook)" '"queued":'
has "clear caches" "$(A -X POST $B/api/v1/admin/maintenance/clear-caches)" '"cleared":true'
check "unknown task 404" "$(A -o /dev/null -w '%{http_code}' -X POST $B/api/v1/admin/maintenance/explode)" 404
for _ in $(seq 20); do grep -q '<span class="hl-' <<<"$(curl -s "$B/topic/$WT-watched-forum-topic/page/2/")$(curl -s "$B/topic/$WT-watched-forum-topic/")" && break; sleep 0.5; done
has "re-cooked posts still highlighted" "$(curl -s "$B/topic/$WT-watched-forum-topic/")$(curl -s "$B/topic/$WT-watched-forum-topic/page/2/")" 'class="hl-'
A -o /dev/null -X PUT -d '{"value":"Quiet Forum"}' $B/api/v1/admin/settings/site_name
EXP=$(A $B/api/v1/admin/settings-export)
has "export holds changed settings" "$EXP" '"site_name":"Quiet Forum"'
check "import rejects bad values" "$(A -o /dev/null -w '%{http_code}' -X POST -d '{"posts_per_page":9999}' $B/api/v1/admin/settings-import)" 422
check "import applies" "$(A -o /dev/null -w '%{http_code}' -X POST -d '{"site_name":"Boring Forum"}' $B/api/v1/admin/settings-import)" 200
has "imported value live" "$(curl -s $B/)" "<title>Boring Forum</title>"
PC=$(A "$B/api/v1/admin/permission-check?user=frank&forum=$OFF")
has "permission check result" "$PC" '"result":["view","read","create","reply","attach"]'
has "permission check explains groups" "$PC" '"group":2'
check "take site offline" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"value":true}' $B/api/v1/admin/settings/offline)" 204
check "guests see offline page" "$(curl -s -o /dev/null -w '%{http_code}' $B/)" 503
has "offline message" "$(curl -s $B/)" "We are doing some maintenance"
check "login still reachable" "$(curl -s -o /dev/null -w '%{http_code}' $B/login)" 200
check "admins still use the site" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/)" 200
check "bring site back" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"value":false}' $B/api/v1/admin/settings/offline)" 204
check "guests are back" "$(curl -s -o /dev/null -w '%{http_code}' $B/)" 200

# --- notifications: grouped likes, open/mark read, staff edits, unsubscribe, pending users
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Notification playground" --data-urlencode "body=A fresh topic for notifications" $B/forum/$FORUM/new)
NTP=$(grep -oE '/topic/[0-9]+' <<<"$LOC" | grep -oE '[0-9]+')
read N1 NT1 <<<"$(newuser nina)"; read N2 NT2 <<<"$(newuser noah)"
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $N1 --data "csrf=$NT1&body=Nina writes a likeable post" $B/topic/$NTP/reply)
NPID=$(grep -oE 'data-quote="[0-9]+"' <<<"$(curl -s -b $J "$LOC")" | tail -1 | grep -oE '[0-9]+')
curl -s -o /dev/null -b $J --data "csrf=$T" $B/post/$NPID/like
curl -s -o /dev/null -b $N2 --data "csrf=$NT2" $B/post/$NPID/like
NN=$(curl -s -b $N1 "$B/notifications?show=likes")
has "likes are grouped" "$NN" "noah and 1 other liked your post"
has "header badge counts" "$(curl -s -b $N1 $B/)" 'data-island="notify-count"'
OPEN=$(grep -oE 'href="/notifications/[0-9]+/open"' <<<"$NN" | head -1 | grep -oE '/notifications/[0-9]+/open')
has "opening goes to the post" "$(curl -s -o /dev/null -w '%{redirect_url}' -b $N1 $B$OPEN)" "#post-"
check "others cannot open it" "$(curl -s -o /dev/null -w '%{http_code}' -b $N2 $B$OPEN)" 404
check "mark all read" "$(curl -s -o /dev/null -w '%{http_code}' -b $N1 --data "csrf=$NT1" $B/notifications/read)" 303
if grep -q 'class="unread"' <<<"$(curl -s -b $N1 $B/notifications)"; then echo "FAIL still unread"; fail=1; else echo "ok   all read"; fi
check "staff edit" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Nina writes a likeable post (tidied)&reason=tidy" $B/post/$NPID/edit)" 303
has "author told about staff edit" "$(curl -s -b $N1 "$B/notifications?show=staff")" "edited your post"
curl -s -o /dev/null -b $N1 --data "csrf=$NT1&level=watch" $B/topic/$NTP/follow
curl -s -o /dev/null -b $N1 --data "csrf=$NT1&reply_app=on&reply_email=on&mention_app=on" $B/settings/notifications
curl -s -o /dev/null -b $N2 --data "csrf=$NT2&body=Noah replies to wake watchers" $B/topic/$NTP/reply
if [ -n "$MAIL" ]; then
  for _ in $(seq 20); do grep -ql "To: nina@example.com" "$MAIL"/*.eml 2>/dev/null && grep -l "To: nina@example.com" "$MAIL"/*.eml | xargs grep -ql "List-Unsubscribe" && break; sleep 0.5; done
  NM=$(grep -l "To: nina@example.com" "$MAIL"/*.eml | xargs grep -l "List-Unsubscribe" | tail -1 | xargs cat)
  has "list-unsubscribe header" "$NM" "List-Unsubscribe-Post: List-Unsubscribe=One-Click"
  UNS=$(grep -oE 'https?://[^ >]*/unsubscribe/[0-9a-f-]+' <<<"$NM" | head -1); UNS=${UNS/http:\/\/127.0.0.1:8099/$B}
  has "unsubscribe confirm page" "$(curl -s "$UNS")" "Stop these emails?"
  check "one-click unsubscribe" "$(curl -s -o /dev/null -w '%{http_code}' -X POST -d "List-Unsubscribe=One-Click" "$UNS")" 200
  if grep -q 'name="reply_email" aria-label="New posts in topics you watch: email" checked' <<<"$(curl -s -b $N1 $B/settings)"; then echo "FAIL still subscribed"; fail=1; else echo "ok   email off after unsubscribe"; fi
fi
check "bad unsubscribe token" "$(curl -s -o /dev/null -w '%{http_code}' -X POST $B/unsubscribe/nope)" 404
A -o /dev/null -X PUT -d '{"value":true}' $B/api/v1/admin/settings/require_account_approval
read P1J P1T <<<"$(newuser petra)"
has "pending user in review queue" "$(curl -s -b $J $B/review)" "Accounts awaiting approval"
PUID=$(A "$B/api/v1/admin/users?q=petra" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "approve from queue" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&decision=approve" $B/review/user/$PUID)" 303
check "approved petra posts" "$(curl -s -o /dev/null -w '%{http_code}' -b $P1J --data "csrf=$P1T&body=Petra is approved now" $B/topic/$NTP/reply)" 303
A -o /dev/null -X PUT -d '{"value":false}' $B/api/v1/admin/settings/require_account_approval
rm -f $N1 $N2 $P1J

# --- edit history: conflicts, diffs, hidden revisions, revert, edit lock, time limit
read OJ OT <<<"$(newuser olga)"
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $OJ --data-urlencode "csrf=$OT" --data-urlencode "title=Olga edits a topic" --data-urlencode "body=The quick brown fox jumps" $B/forum/$FORUM/new)
OP=$(grep -oE 'href="/post/[0-9]+/edit"' <<<"$(curl -s -b $OJ "$LOC")" | head -1 | grep -oE '[0-9]+')
check "staff edit (revision 1)" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&base=0&body=The quick red fox jumps&reason=colour" $B/post/$OP/edit)" 303
check "stale form is a conflict" "$(curl -s -o /dev/null -w '%{http_code}' -b $OJ --data "csrf=$OT&base=0&body=Olga overwrites everything&reason=x" $B/post/$OP/edit)" 409
H1=$(curl -s -b $OJ $B/post/$OP/history)
has "word diff shown" "$H1" "<del>brown</del><ins>red</ins>"
check "hide revision" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&on=1" $B/post/$OP/revisions/1/hide)" 303
has "members see it is hidden" "$(curl -s -b $OJ $B/post/$OP/history)" "hidden by staff"
has "api withholds hidden text" "$(curl -s -b $OJ $B/api/v1/posts/$OP/revisions)" '"raw":null'
check "members cannot hide" "$(curl -s -o /dev/null -w '%{http_code}' -b $OJ --data "csrf=$OT&on=0" $B/post/$OP/revisions/1/hide)" 403
curl -s -o /dev/null -b $J --data "csrf=$T&on=0" $B/post/$OP/revisions/1/hide
check "revert" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T" $B/post/$OP/revisions/1/revert)" 303
has "reverted text is live" "$(curl -s "$LOC")" "The quick brown fox jumps"
check "lock against author edits" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&on=1" $B/post/$OP/edit-lock)" 303
check "author cannot edit locked post" "$(curl -s -o /dev/null -w '%{http_code}' -b $OJ --data "csrf=$OT&body=Trying to edit a locked post" $B/post/$OP/edit)" 403
curl -s -o /dev/null -b $J --data "csrf=$T&on=0" $B/post/$OP/edit-lock
A -o /dev/null -X PUT -d '{"value":true}' $B/api/v1/admin/settings/require_edit_reason
check "reason required after grace" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Edit with no reason given" $B/post/$OP/edit)" 422
A -o /dev/null -X PUT -d '{"value":false}' $B/api/v1/admin/settings/require_edit_reason
sqlnow=0
A -o /dev/null -X PUT -d '{"value":1}' $B/api/v1/admin/settings/edit_time_limit_minutes
check "author inside time limit" "$(curl -s -o /dev/null -w '%{http_code}' -b $OJ --data "csrf=$OT&body=Olga edits within the limit" $B/post/$OP/edit)" 303
A -o /dev/null -X PUT -d '{"value":0}' $B/api/v1/admin/settings/edit_time_limit_minutes
rm -f $OJ

# --- bans, blocked domains, link limit, flood wait
check "cannot ban own address" "$(A -o /dev/null -w '%{http_code}' -d '{"kind":"ip","value":"127.0.0.0/8"}' $B/api/v1/admin/bans)" 422
check "ban a network" "$(A -o /dev/null -w '%{http_code}' -d '{"kind":"ip","value":"10.0.0.0/8","reason":"abuse"}' $B/api/v1/admin/bans)" 201
check "bad ban rejected" "$(A -o /dev/null -w '%{http_code}' -d '{"kind":"ip","value":"not-an-ip"}' $B/api/v1/admin/bans)" 422
check "ban an email domain" "$(A -o /dev/null -w '%{http_code}' -d '{"kind":"email","value":"@spam.example"}' $B/api/v1/admin/bans)" 201
has "bans listed" "$(A $B/api/v1/admin/bans)" '"value":"10.0.0.0/8"'
X=$(mktemp); curl -s -c $X -b $X $B/register >/dev/null; TX=$(csrf $X)
check "banned email domain refused" "$(curl -s -o /dev/null -w '%{http_code}' -c $X -b $X --data "csrf=$TX&username=spamz&email=a@spam.example&password=spam long passphrase&accept=1&field_$FLD=Go" $B/register)" 422
A -o /dev/null -X PUT -d '{"value":"mailinator.com, trash.example"}' $B/api/v1/admin/settings/blocked_email_domains
check "disposable domain refused" "$(curl -s -o /dev/null -w '%{http_code}' -c $X -b $X --data "csrf=$TX&username=temp1&email=t@trash.example&password=temp long passphrase&accept=1&field_$FLD=Go" $B/register)" 422
A -o /dev/null -X PUT -d '{"value":""}' $B/api/v1/admin/settings/blocked_email_domains
rm -f $X
BID=$(A $B/api/v1/admin/bans | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "remove ban" "$(A -o /dev/null -w '%{http_code}' -X DELETE $B/api/v1/admin/bans/$BID)" 204
read QJ QT <<<"$(newuser quinn)"
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Limits playground topic" --data-urlencode "body=A topic to test posting limits" $B/forum/$FORUM/new)
LT=$(grep -oE '/topic/[0-9]+' <<<"$LOC" | grep -oE '[0-9]+')
A -o /dev/null -X PUT -d '{"value":5}' $B/api/v1/admin/settings/new_user_posts
check "new member link limit" "$(curl -s -o /dev/null -w '%{http_code}' -b $QJ --data-urlencode "csrf=$QT" --data-urlencode "body=see https://a.example https://b.example https://c.example" $B/topic/$LT/reply)" 422
check "two links are fine" "$(curl -s -o /dev/null -w '%{http_code}' -b $QJ --data-urlencode "csrf=$QT" --data-urlencode "body=see https://a.example and https://b.example" $B/topic/$LT/reply)" 303
A -o /dev/null -X PUT -d '{"value":0}' $B/api/v1/admin/settings/new_user_posts
A -o /dev/null -X PUT -d '{"value":60}' $B/api/v1/admin/settings/flood_wait_seconds
check "flood wait" "$(curl -s -o /dev/null -w '%{http_code}' -b $QJ --data "csrf=$QT&body=Posting again right away" $B/topic/$LT/reply)" 422
check "admins skip flood wait" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&body=Admin posting quickly" $B/topic/$LT/reply)" 303
A -o /dev/null -X PUT -d '{"value":0}' $B/api/v1/admin/settings/flood_wait_seconds
rm -f $QJ

# --- i18n base and interface preferences
PLH=$(curl -s -H 'Accept-Language: pl-PL,pl;q=0.9' $B/)
has "polish from accept-language" "$PLH" '<html lang="pl"'
has "polish chrome" "$PLH" '>Fora</a>'
grep -qE '[0-9]+ (temat|tematy|tematów)<' <<<"$PLH" && echo "ok   polish plural forms" || { echo "FAIL polish plurals"; fail=1; }
ENH=$(curl -s -H 'Accept-Language: en-US' $B/)
has "english not served from polish cache" "$ENH" '<html lang="en"'
has "vary header" "$(curl -s -D - -o /dev/null $B/)" "vary: Accept-Language, Cookie"
LC=$(mktemp)
curl -s -o /dev/null -c $LC -b $LC $B/language/pl
has "language cookie" "$(curl -s -b $LC $B/)" '<html lang="pl"'
check "unknown language 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/language/xx)" 404
rm -f $LC
check "save interface" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&locale=pl&text_size=110&density=compact" $B/settings/interface)" 303
MEH=$(curl -s -b $J $B/)
has "member language" "$MEH" '<html lang="pl" dir="ltr" data-density="compact" data-text-size="110">'
check "bad interface values" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&locale=xx&text_size=500&density=tiny" $B/settings/interface)" 422
curl -s -o /dev/null -b $J --data "csrf=$T&locale=en&text_size=100&density=comfortable" $B/settings/interface

# --- profiles: display name, title, last seen privacy, tabs, cover, hover card
read RJ RT <<<"$(newuser rosa)"
RID=$(A "$B/api/v1/admin/users?q=rosa" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "save display name" "$(curl -s -o /dev/null -w '%{http_code}' -b $RJ --data-urlencode "csrf=$RT" --data-urlencode "display_name=Rosa R." --data "hide_last_seen=1&field_$FLD=Go" $B/settings/profile)" 303
check "admin sets title" "$(A -o /dev/null -w '%{http_code}' -X PATCH -d '{"title":"Helper"}' $B/api/v1/admin/users/$RID)" 204
curl -s -o /dev/null -b $RJ $B/
RP=$(curl -s $B/profile/$RID-rosa/)
has "display name shown" "$RP" "Rosa R. <span class=\"muted small\">@rosa</span>"
has "title shown" "$RP" "Helper"
if grep -q "Last seen" <<<"$RP"; then echo "FAIL last seen not hidden"; fail=1; else echo "ok   last seen hidden from others"; fi
has "staff still see last seen" "$(curl -s -b $J $B/profile/$RID-rosa/)" "Last seen"
has "likes received" "$RP" "Likes received"
has "topics tab" "$(curl -s "$B/profile/1-alicia/?tab=topics")" "Topics started"
curl -s -o /dev/null -b $RJ -X PUT -H "X-CSRF-Token: $RT" -H 'Content-Type: application/json' -d '{"title":"Rosa half-written topic","body":"draft text"}' "$B/api/v1/drafts?context=new-topic:$FORUM"
has "own drafts tab" "$(curl -s -b $RJ "$B/profile/$RID-rosa/?tab=drafts")" "Rosa half-written topic"
if grep -q "Rosa half-written topic" <<<"$(curl -s -b $J "$B/profile/$RID-rosa/?tab=drafts")"; then echo "FAIL drafts visible to others"; fail=1; else echo "ok   drafts are private"; fi
TMPC=$(mktemp -d)
printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==' | base64 -d > $TMPC/cover.png
check "upload cover" "$(curl -s -o /dev/null -w '%{http_code}' -b $RJ -F "csrf=$RT" -F "file=@$TMPC/cover.png" $B/settings/cover)" 303
has "cover shown" "$(curl -s $B/profile/$RID-rosa/)" 'class="cover"'
rm -rf $TMPC
CARD=$(curl -s $B/api/v1/users/rosa/card)
has "hover card" "$CARD" '"display_name":"Rosa R."'
check "unknown card 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/v1/users/nobody-here/card)" 404
rm -f $RJ

# --- cooking extras, mention limit, duplicate links
CB=$(printf '# Big heading\n\nClaim[^1]\n\n[^1]: Source\n\n```spoiler\nhidden **text**\n```\n\nhttps://dup.example/page\n')
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Cooking extras topic" --data-urlencode "body=$CB" $B/forum/$FORUM/new)
CP=$(curl -s "$LOC")
has "heading anchor" "$CP" 'id="h-big-heading"'
has "footnote" "$CP" 'href="#fn-1"'
has "spoiler" "$CP" '<details><summary>Spoiler</summary>'
has "link already posted" "$(A -d '{"urls":["https://dup.example/page","https://new.example"]}' $B/api/v1/links/seen)" '"title":"Cooking extras topic"'
check "link check needs a member" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d '{"urls":[]}' $B/api/v1/links/seen)" 403
A -o /dev/null -X PUT -d '{"value":0}' $B/api/v1/admin/settings/max_mentions_per_post
read MJ MT <<<"$(newuser mona)"
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode "title=Mention limit topic" --data-urlencode "body=Hello there @mona friend" $B/forum/$FORUM/new
if grep -q "mentioned you in Mention limit" <<<"$(curl -s -b $MJ $B/notifications)"; then echo "FAIL mention limit"; fail=1; else echo "ok   mention limit respected"; fi
A -o /dev/null -X PUT -d '{"value":10}' $B/api/v1/admin/settings/max_mentions_per_post
rm -f $MJ

# --- topic states, reasons, change author, bookmarks
read VJ VT <<<"$(newuser vera)"
LOC=$(curl -s -o /dev/null -w '%{redirect_url}' -b $VJ --data-urlencode "csrf=$VT" --data-urlencode "title=States playground topic" --data-urlencode "body=Topic for the state checks" $B/forum/$FORUM/new)
ST=$(grep -oE '/topic/[0-9]+' <<<"$LOC" | grep -oE '[0-9]+')
check "archive with reason" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" --data "action=archive" --data-urlencode "reason=old news" $B/topic/$ST/mod)" 303
AP=$(curl -s "$LOC")
has "archived tag" "$AP" '<span class="tag">Archived</span>'
has "reason in action entry" "$AP" "archived: old news"
check "no replies when archived" "$(curl -s -o /dev/null -w '%{http_code}' -b $VJ --data "csrf=$VT&body=Replying to an archived topic" $B/topic/$ST/reply)" 403
curl -s -o /dev/null -b $J --data "csrf=$T&action=unarchive" $B/topic/$ST/mod
check "feature" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&action=feature" $B/topic/$ST/mod)" 303
has "featured tag" "$(curl -s "$LOC")" '<span class="tag">Featured</span>'
check "members cannot change author" "$(curl -s -o /dev/null -w '%{http_code}' -b $VJ --data "csrf=$VT&username=frank" $B/topic/$ST/author)" 403
check "unknown new author" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&username=nobody-here" $B/topic/$ST/author)" 422
check "change author" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data "csrf=$T&username=frank" $B/topic/$ST/author)" 303
has "new author shown" "$(curl -s "$LOC")" "<strong>frank</strong>"
BP=$(grep -oE 'action="/post/[0-9]+/bookmark"' <<<"$(curl -s -b $VJ "$LOC")" | head -1 | grep -oE '[0-9]+')
check "bookmark" "$(curl -s -o /dev/null -w '%{http_code}' -b $VJ --data "csrf=$VT" $B/post/$BP/bookmark)" 303
VID=$(A "$B/api/v1/admin/users?q=vera" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
has "bookmarks tab" "$(curl -s -b $VJ "$B/profile/$VID-vera/?tab=bookmarks")" "States playground topic"
if grep -q "States playground topic" <<<"$(curl -s -b $J "$B/profile/$VID-vera/?tab=bookmarks")"; then echo "FAIL bookmarks public"; fail=1; else echo "ok   bookmarks are private"; fi
rm -f $VJ

# --- magic links, forum rules, copy permissions
check "magic link off by default" "$(curl -s -o /dev/null -w '%{http_code}' $B/login/magic)" 404
A -o /dev/null -X PUT -d '{"value":true}' $B/api/v1/admin/settings/allow_magic_link
read GJ GT <<<"$(newuser gina)"
G2=$(mktemp); curl -s -c $G2 -b $G2 $B/login/magic >/dev/null; TG2=$(csrf $G2)
has "magic link request" "$(curl -s -c $G2 -b $G2 --data "csrf=$TG2&email=gina@example.com" $B/login/magic)" "a sign-in link is on its way"
if [ -n "$MAIL" ]; then
  ML=$(mail_link gina@example.com /login/magic/); ML=${ML/http:\/\/127.0.0.1:8099/$B}
  check "opening link does not use it" "$(curl -s -o /dev/null -w '%{http_code}' -b $G2 "$ML")" 200
  check "magic link signs in" "$(curl -s -o /dev/null -w '%{http_code}' -c $G2 -b $G2 --data "csrf=$TG2" "$ML")" 303
  has "signed in as gina" "$(curl -s -b $G2 $B/settings)" "gina@example.com"
  check "magic link single use" "$(curl -s -o /dev/null -w '%{http_code}' -c $G2 -b $G2 --data "csrf=$TG2" "$ML")" 410
fi
A -o /dev/null -X PUT -d '{"value":false}' $B/api/v1/admin/settings/allow_magic_link
rm -f $GJ $G2
check "set forum rules" "$(A -o /dev/null -w '%{http_code}' -X PUT -d '{"rules":"Be **nice** here"}' $B/api/v1/admin/nodes/$FORUM/rules)" 204
has "rules shown" "$(curl -s "$B/forum/$FORUM-introductions/")" "Be <strong>nice</strong> here"
check "copy permissions" "$(A -o /dev/null -w '%{http_code}' -X POST -d "{\"from\":$FORUM}" $B/api/v1/admin/nodes/$OFF/permissions/copy)" 204
check "copied: guests can see the forum" "$(curl -s -o /dev/null -w '%{http_code}' "$B/forum/$OFF-off-topic/")" 200

check "logout" "$(curl -s -o /dev/null -w '%{http_code}' -c $K -b $K --data "csrf=$T2" $B/logout)" 303
check "logged-out bob is a guest" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/notifications)" 403
check "wrong password" "$(curl -s -o /dev/null -w '%{http_code}' -c $K -b $K --data "csrf=$(csrf $K)&login=bob&password=wrongwrongwrong" $B/login)" 422

# --- composer autocomplete, navigation editor, bounces, topic islands
check "autocomplete needs login" "$(curl -s -o /dev/null -w '%{http_code}' "$B/api/v1/autocomplete?kind=user&q=al")" 403
has "autocomplete member" "$(curl -s -b $J "$B/api/v1/autocomplete?kind=user&q=ro")" '"label":"rosa"'
has "autocomplete forum" "$(curl -s -b $J "$B/api/v1/autocomplete?kind=node&q=intro")" '"label":"Introductions"'
check "autocomplete wildcard is literal" "$(curl -s -b $J "$B/api/v1/autocomplete?kind=user&q=%25")" "[]"
has "topic has scrubber island" "$(curl -s -b $J $B/topic/1-hello-from-alice/)" 'data-island="scrubber"'
has "topic has multi-quote" "$(curl -s -b $J $B/topic/1-hello-from-alice/)" 'data-multiquote='
T=$(csrf $J)
NAV='[{"key":"latest"},{"label":"Docs","url":"/docs-link"},{"label":"Bad","url":"javascript:alert(1)"},{"label":"Staff only","url":"/staff-link","group":3}]'
check "admin saves nav" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X PUT -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' --data "$(node -e 'console.log(JSON.stringify({value:process.argv[1]}))' "$NAV")" $B/api/v1/admin/settings/nav_links)" 204
settle 3
NAVHOME=$(curl -s -H 'Cookie: x=1' $B/search)
has "custom nav link" "$NAVHOME" 'href="/docs-link"'
no() { if grep -qF -- "$3" <<<"$2"; then echo "FAIL $1 (found '$3')"; fail=1; else echo "ok   $1"; fi; }
no "unsafe nav link dropped" "$NAVHOME" 'javascript:alert'
no "group nav link hidden from guests" "$NAVHOME" '/staff-link'
has "admin sees group nav link" "$(curl -s -b $J $B/search)" '/staff-link'
curl -s -o /dev/null -b $J -X PUT -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"value":""}' $B/api/v1/admin/settings/nav_links
if [ -n "${BF_BOUNCE_TOKEN:-}" ]; then
  hook() { curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $1" -H 'Content-Type: application/json' -d '{"email":"bob@example.com"}' $B/api/v1/webhooks/bounce; }
  check "bounce needs token" "$(hook wrong)" 403
  for _ in 1 2; do check "bounce counted" "$(hook "$BF_BOUNCE_TOKEN")" 204; done
  no "not bad before the limit" "$(curl -s -b $J $B/api/v1/admin/mail/bad)" 'bob@example.com'
  check "third bounce" "$(hook "$BF_BOUNCE_TOKEN")" 204
  has "bad after the limit" "$(curl -s -b $J $B/api/v1/admin/mail/bad)" 'bob@example.com'
  BID=$(curl -s -b $J $B/api/v1/admin/mail/bad | grep -oE '"id":[0-9]+' | head -1 | cut -d: -f2)
  check "admin clears bad address" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X DELETE -H "X-CSRF-Token: $T" $B/api/v1/admin/mail/bad/$BID)" 204
fi

# --- new P1 items: Apple, thumbnails, group mentions, infinite scroll, scrubber dates, bounce formats
if [ -n "${BF_OAUTH_APPLE_ID:-}" ]; then
  has "login page offers apple" "$(curl -s $B/login)" 'action="/auth/apple"'
  A=$(mktemp)
  curl -s -o /dev/null -X POST -d '{"sub":"apple-1","email":"apple.user@example.com","email_verified":true}' $OIDC/next-user
  AUTH=$(curl -s -c $A -b $A -o /dev/null -w '%{redirect_url}' $B/auth/apple)
  has "apple asks for form_post" "$AUTH" 'response_mode=form_post'
  R=$(curl -s "$AUTH")
  CODE=$(node -e 'console.log(JSON.parse(process.argv[1]).code)' "$R"); ST=$(node -e 'console.log(JSON.parse(process.argv[1]).state)' "$R")
  HOP=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' --data-urlencode "code=$CODE" --data-urlencode "state=$ST" --data-urlencode 'user={"name":{"firstName":"Appleton"}}' $B/auth/apple/callback)
  has "apple form post hops to GET" "$HOP" "/auth/apple/callback?code=$CODE"
  check "apple signs in" "$(curl -s -L -o /dev/null -w '%{url_effective}' -c $A -b $A "$B/auth/apple/callback?code=$CODE&state=$ST&name=Appleton")" "$B/"
  has "apple account created" "$(curl -s -b $A $B/settings)" "apple.user@example.com"
  check "form post refused for other providers" "$(curl -s -o /dev/null -w '%{http_code}' -d 'code=x' $B/auth/oidc/callback)" 404
  rm -f $A
fi
T=$(csrf $J)
TMPI=$(mktemp -d)
node -e '
const zlib=require("zlib");const w=1200,h=600;const raw=Buffer.alloc((w*3+1)*h);
const crc=(b)=>{let c,r=~0;for(const x of b){c=(r^x)&255;for(let k=0;k<8;k++)c=c&1?c>>>1^0xedb88320:c>>>1;r=r>>>8^c}return ~r>>>0};
const chunk=(t,d)=>{const l=Buffer.alloc(4);l.writeUInt32BE(d.length);const b=Buffer.concat([Buffer.from(t),d]);const c=Buffer.alloc(4);c.writeUInt32BE(crc(b));return Buffer.concat([l,b,c])};
const ih=Buffer.alloc(13);ih.writeUInt32BE(w,0);ih.writeUInt32BE(h,4);ih[8]=8;ih[9]=2;
require("fs").writeFileSync(process.argv[1],Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]),chunk("IHDR",ih),chunk("IDAT",zlib.deflateSync(raw)),chunk("IEND",Buffer.alloc(0))]));' "$TMPI/big.png"
UP=$(curl -s -b $J -H "X-CSRF-Token: $T" -F "file=@$TMPI/big.png" $B/api/v1/uploads)
has "upload markdown carries size" "$UP" '#1200x600'
BIG=$(grep -oE '/uploads/[0-9a-f]{64}' <<<"$UP" | head -1)
check "thumbnail served" "$(curl -s -o $TMPI/t.png -w '%{http_code}' $B$BIG-480.png)" 200
check "thumbnail is 480 wide" "$(node -e 'console.log(require("fs").readFileSync(process.argv[1]).readUInt32BE(16))' $TMPI/t.png)" 480
check "unlisted thumbnail width refused" "$(curl -s -o /dev/null -w '%{http_code}' $B$BIG-481.png)" 404
has "preview adds srcset" "$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d "{\"raw\":\"![x]($BIG.png#1200x600)\"}" $B/api/v1/preview)" 'srcset'
rm -rf $TMPI
has "scrubber dates" "$(curl -s $B/api/v1/topics/1/dates)" '[1,"20'
check "infinite scroll needs csrf" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X PUT -H 'Content-Type: application/json' -d '{"on":true}' $B/api/v1/me/infinite-scroll)" 400
check "infinite scroll saved" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X PUT -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"on":true}' $B/api/v1/me/infinite-scroll)" 204
has "infinite scroll in page" "$(curl -s -b $J $B/topic/1-hello-from-alice/)" '{&quot;on&quot;:true}'
has "staff can complete groups" "$(curl -s -b $J "$B/api/v1/autocomplete?kind=user&q=moder")" '"group":true'
if [ -n "${BF_BOUNCE_TOKEN:-}" ]; then
  check "bounce token in query, text/plain body" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: text/plain' -d '{"RecordType":"Bounce","Type":"SoftBounce","Email":"bob@example.com"}' "$B/api/v1/webhooks/bounce?token=$BF_BOUNCE_TOKEN")" 204
fi

# --- tags, search syntax and logs, per-forum settings, bbcode, click counts
T=$(csrf $J)
post_form() { curl -s -o /dev/null -w '%{http_code}' -b $J "$@"; }
TF=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode "title=Tagged topic about zebras" --data-urlencode "body=Zebras [b]bold bbcode[/b] https://zebra.example/page here" --data-urlencode "tags=Stripes, Black White" $B/forum/2/new)
has "tagged topic created" "$TF" "/topic/"
TP=$(curl -s -b $J "$TF")
has "tag shown on topic" "$TP" 'href="/tag/stripes"'
has "bbcode converted" "$TP" '<strong>bold bbcode</strong>'
TID=$(grep -oE '/topic/[0-9]+' <<<"$TF" | head -1 | cut -d/ -f3)
settle 2.5
has "tag page lists topic" "$(curl -s $B/tag/stripes)" 'Tagged topic about zebras'
check "unknown tag 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/tag/nope)" 404
has "forum tag filter" "$(curl -s "$B/forum/2-introductions/?tag=stripes")" 'Tagged topic about zebras'
no "forum tag filter excludes" "$(curl -s "$B/forum/2-introductions/?tag=black-white&after=2999-01-01")" 'Tagged topic about zebras'
has "search tags: operator" "$(curl -s "$B/search?q=zebras+tags:stripes")" 'Tagged topic about zebras'
no "search tags: excludes" "$(curl -s "$B/search?q=zebras+tags:other")" 'Tagged topic about zebras'
has "search operators only" "$(curl -s "$B/search?q=tags:stripes")" 'Tagged topic about zebras'
no "search before: excludes" "$(curl -s "$B/search?q=zebras+before:2000-01-01")" 'Tagged topic about zebras'
has "search in: bookmarks needs a bookmark" "$(curl -s -b $J "$B/search?q=zebras+in:mine")" 'Tagged topic about zebras'
has "search inside a topic" "$(curl -s "$B/search?q=zebras&topic=$TID")" 'Tagged topic about zebras'
no "search other topic" "$(curl -s "$B/search?q=zebras&topic=1")" 'Tagged topic about zebras'
has "live search suggests tags" "$(curl -s "$B/api/v1/search?q=stripes")" '/tag/stripes'
has "search logged" "$(curl -s -b $J $B/api/v1/admin/search-report)" 'zebras'
has "admin lists tags" "$(curl -s -b $J $B/api/v1/admin/tags)" '"slug":"stripes"'
check "author edits tags" "$(post_form --data-urlencode "csrf=$T" --data-urlencode 'tags=Stripes' $B/topic/$TID/tags)" 303
check "others cannot edit tags" "$(curl -s -o /dev/null -w '%{http_code}' -c $K -b $K $B/login >/dev/null; curl -s -o /dev/null -w '%{http_code}' -X POST --data "csrf=x" $B/topic/$TID/tags)" 400
PID=$(curl -s -b $J "$TF" | grep -oE 'data-post="[0-9]+"' | head -1 | grep -oE '[0-9]+')
check "link click counted" "$(curl -s -o /dev/null -w '%{http_code}' -d "{\"post\":$PID,\"url\":\"https://zebra.example/page\"}" $B/api/v1/links/click)" 204
has "click count readable" "$(curl -s $B/api/v1/topics/$TID/links)" '"clicks":1'
curl -s -o /dev/null -b $J -X PATCH -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"default_sort":"views","seo_title":"Custom SEO Title","seo_description":"Custom description","list_style":"list"}' $B/api/v1/admin/nodes/2
settle 2.5
FP=$(curl -s $B/forum/2-introductions/)
has "forum seo title" "$FP" 'Custom SEO Title'
has "forum seo description" "$FP" 'content="Custom description"'
has "forum list style" "$FP" 'list-list'
no "default sort is canonical view, indexable" "$FP" 'name="robots" content="noindex"'
has "unread filter offered" "$(curl -s -b $J $B/forum/2-introductions/)" 'show=unread'
check "unread filter works" "$(curl -s -o /dev/null -w '%{http_code}' -b $J "$B/forum/2-introductions/?show=unread")" 200

# --- moderator panel, topic copy/date/redirect, admin ops, logs, edit history rules
T=$(csrf $J)
CP=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode "title=Copied zebras" -d to=2 $B/topic/$TID/copy)
has "topic copied" "$CP" '/topic/'
has "copy has the posts" "$(curl -s -b $J "$CP")" 'bold bbcode'
CPID=$(grep -oE '/topic/[0-9]+' <<<"$CP" | cut -d/ -f3)
check "start date changed" "$(post_form --data-urlencode "csrf=$T" -d date=2020-02-03 $B/topic/$CPID/date)" 303
check "bad date refused" "$(post_form --data-urlencode "csrf=$T" -d date=nope $B/topic/$CPID/date)" 422
F3=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"forum","title":"Elsewhere","parent_id":1}' $B/api/v1/admin/nodes | grep -oE '[0-9]+' | head -1)
check "move leaves link" "$(post_form --data-urlencode "csrf=$T" -d "to=$F3&redirect=1&redirect_days=2" $B/topic/$CPID/move)" 303
settle 2.5
has "redirect listed in old forum" "$(curl -s $B/forum/2-introductions/)" 'Moved: <a'
check "modcp for staff" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/modcp)" 200
check "modcp refused for guests" "$(curl -s -o /dev/null -w '%{http_code}' $B/modcp)" 403
has "ip tool finds member" "$(curl -s -b $J "$B/modcp?ip=127.0.0.1")" '<td class="num">'
check "ip tool rejects junk" "$(curl -s -o /dev/null -w '%{http_code}' -b $J "$B/modcp?ip=';drop")" 422
has "member ips" "$(curl -s -b $J $B/api/v1/admin/users/1/ips)" '127.0.0.1'
has "dashboard" "$(curl -s -b $J $B/api/v1/admin/dashboard)" '"version"'
check "test mail" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"to":"ops@example.com"}' $B/api/v1/admin/mail/test)" 202
check "task: rebuild search" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/maintenance/rebuild-search)" 200
check "task: housekeeping" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/maintenance/housekeeping)" 200
has "task log" "$(curl -s -b $J $B/api/v1/admin/logs/tasks)" 'housekeeping'
check "error log readable" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/api/v1/admin/logs/errors)" 200
curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -X PUT -d '{"value":"staff"}' -o /dev/null $B/api/v1/admin/settings/edit_history_visible_to
sleep 1
check "history hidden from guests when staff-only" "$(curl -s -o /dev/null -w '%{http_code}' $B/post/$PID/history)" 403
check "staff still see history" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/post/$PID/history)" 200
curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -X PUT -d '{"value":"everyone"}' -o /dev/null $B/api/v1/admin/settings/edit_history_visible_to
EDIT1=$(curl -s -b $J $B/post/$PID/edit | grep -oE 'name="base" value="[0-9]+"' | grep -oE '[0-9]+')
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode "body=Their version of the text here" --data-urlencode "reason=first" -d base=$EDIT1 $B/post/$PID/edit
CONF=$(curl -s -b $J -w '\n%{http_code}' --data-urlencode "csrf=$T" --data-urlencode "body=My version of the text here" --data-urlencode "reason=second" -d base=$EDIT1 $B/post/$PID/edit)
has "stale edit conflicts" "$(tail -1 <<<"$CONF")" 409
has "conflict shows both sides" "$CONF" 'Their edit'
check "delete revision" "$(post_form --data-urlencode "csrf=$T" $B/post/$PID/revisions/1/delete)" 303
GONE=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"forum","title":"x"}' -o /dev/null -w '' $B/api/v1/admin/nodes)
curl -s -c $K -b $K $B/register >/dev/null; T2=$(csrf $K)
curl -s -o /dev/null -c $K -b $K --data "csrf=$T2&username=temp_user&email=temp@example.com&password=another long passphrase&ts=0" $B/register
TU=$(curl -s -b $J "$B/api/v1/admin/users?q=temp_user" | grep -oE '"id":[0-9]+' | head -1 | cut -d: -f2)
if [ -n "$TU" ]; then
  check "reset 2fa" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/users/$TU/reset-2fa)" 204
  check "delete member" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X DELETE -H "X-CSRF-Token: $T" $B/api/v1/admin/users/$TU)" 204
fi
check "self delete refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X DELETE -H "X-CSRF-Token: $T" $B/api/v1/admin/users/1)" 422

# --- security headers, invites, captcha, prefs, emoji, templates, reply context, image limit
H=$(curl -s -D - -o /dev/null $B/)
has "csp header" "$H" "script-src 'self' 'sha256-"
has "csp has no unsafe-inline scripts" "$(grep -i '^content-security-policy' <<<"$H" | grep -o "script-src [^;]*")" "sha256"
has "coop" "$H" "same-origin"
has "permissions policy" "$H" "camera=()"
T=$(csrf $J)
check "cross-site write refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H 'Sec-Fetch-Site: cross-site' --data-urlencode "csrf=$T" $B/read-all)" 403
check "foreign origin refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H 'Origin: https://evil.example' --data-urlencode "csrf=$T" $B/read-all)" 403
check "own origin allowed" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "Origin: $B" --data-urlencode "csrf=$T" $B/read-all)" 303
check "csp reports accepted" "$(curl -s -o /dev/null -w '%{http_code}' -d '{}' $B/api/v1/csp-report)" 204
setting() { curl -s -o /dev/null -b $J -X PUT -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d "{\"value\":$2}" $B/api/v1/admin/settings/$1; }
reg() { local C=$(mktemp); curl -s -c $C -b $C -o /dev/null $B/register; curl -s -o /dev/null -w '%{http_code}' -c $C -b $C --data-urlencode "csrf=$(csrf $C)" --data-urlencode "username=$1" --data-urlencode "email=$1@example.com" --data-urlencode "password=another long passphrase" --data-urlencode "ts=0" --data-urlencode "accept=1" --data-urlencode "birth_date=1990-01-01" --data-urlencode "field_$FLD=Go" "${@:2}" $B/register; rm -f $C; }
setting min_register_seconds 0
setting invite_only true
settle 1.5
has "register asks for invite" "$(curl -s $B/register)" 'name="invite"'
check "no invite refused" "$(reg noinvite)" 422
INV=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"note":"x"}' $B/api/v1/admin/invites)
CODE=$(node -e 'console.log(JSON.parse(process.argv[1]).code)' "$INV")
has "invite link prefills" "$(curl -s "$B/register?invite=$CODE")" "value=\"$CODE\""
check "invite works" "$(reg invited1 --data-urlencode "invite=$CODE")" 303
check "invite is single use" "$(reg invited2 --data-urlencode "invite=$CODE")" 422
setting invite_only false
if [ -n "${BF_CAPTCHA_VERIFY_URL:-}" ]; then
  setting captcha_secret '"s"'
  setting captcha_provider '"turnstile"'; setting captcha_site_key '"site-key"'
  settle 1.5
  has "challenge on page" "$(curl -s $B/register)" 'cf-turnstile'
  has "challenge host in csp" "$(curl -s -D - -o /dev/null $B/register)" 'challenges.cloudflare.com'
  check "failed challenge refused" "$(reg capt1 --data-urlencode 'cf-turnstile-response=bad')" 422
  check "passed challenge" "$(reg capt2 --data-urlencode 'cf-turnstile-response=good')" 303
  setting captcha_provider '""'
  setting captcha_secret '""'
fi
settle 1.5
check "save prefs" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'locale=en&text_size=100&density=comfortable&date_format=iso&skin_tone=3&profile_visibility=nobody' $B/settings/interface)" 303
SP=$(curl -s -b $J $B/settings)
has "date format applied" "$SP" 'data-date="iso"'
has "skin tone applied" "$SP" 'data-skin="3"'
ALICE=$(grep -oE 'class="nav-user" href="/settings">[^<]+' <<<"$SP" | sed 's/.*>//')
PF=$(curl -s -o /dev/null -w '%{http_code}' "$B/u/$ALICE")
PU=$(curl -s -o /dev/null -w '%{redirect_url}' "$B/u/$ALICE")
check "hidden profile refused for guests" "$(curl -s -o /dev/null -w '%{http_code}' "$PU")" 403
check "owner still sees profile" "$(curl -s -o /dev/null -w '%{http_code}' -b $J "$PU")" 200
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" -d 'locale=en&text_size=100&density=comfortable&date_format=relative&skin_tone=0&profile_visibility=everyone' $B/settings/interface
check "emoji needs an upload path" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"name":"x","url":"javascript:alert(1)"}' $B/api/v1/admin/emoji)" 422
check "add emoji" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d "{\"name\":\"party\",\"url\":\"$BIG.png\"}" $B/api/v1/admin/emoji)" 204
has "emoji listed" "$(curl -s $B/api/v1/emoji)" '"party"'
has "emoji cooks" "$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"raw":"hello :party: there"}' $B/api/v1/preview)" "$BIG.png"
check "template needs link" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"verify","locale":"pl","subject":"s","body":"no link"}' $B/api/v1/admin/mail/templates)" 422
check "template saved" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"reset","locale":"pl","subject":"Haslo {site}","body":"Otworz {link}"}' $B/api/v1/admin/mail/templates)" 204
has "template preview" "$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"reset","locale":"pl","subject":"S {username}","body":"{link}"}' $B/api/v1/admin/mail/preview)" 'sample_member'
curl -s -o /dev/null -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -X PUT -d '{"max_images_per_post":1}' $B/api/v1/admin/groups/2/limits
check "asn ban validated" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"asn","value":"not a number"}' $B/api/v1/admin/bans)" 422
check "reply context" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/v1/topics/$TID/posts/1)" 200
check "reply context 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/v1/topics/$TID/posts/999)" 404
has "download counts" "$(curl -s "$B/api/v1/uploads/counts?keys=${BIG##*/}.png")" '"downloads"'

# --- forum rules, watched link/tag, pins, link forums, review tools, muted tags
T=$(csrf $J)
adm() { curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' "$@"; }
check "watched tag word" "$(adm -d '{"action":"tag","pattern":"giraffes","replacement":"animals"}' $B/api/v1/admin/watched-words)" 201
check "watched link word" "$(adm -d '{"action":"link","pattern":"okapi","replacement":"https://okapi.example/"}' $B/api/v1/admin/watched-words)" 201
LF=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"link","title":"Docs site","parent_id":1,"url":"https://docs.example/"}' $B/api/v1/admin/nodes | grep -oE '[0-9]+' | head -1)
AF=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"kind":"archive","title":"Old stuff","parent_id":1}' $B/api/v1/admin/nodes | grep -oE '[0-9]+' | head -1)
settle 2.5
has "link forum goes through counter" "$(curl -s $B/)" "/forum/$LF/go"
check "link forum redirects" "$(curl -s -o /dev/null -w '%{redirect_url}' $B/forum/$LF/go)" "https://docs.example/"
has "link forum counts" "$(curl -s -b $J $B/api/v1/admin/tree)" '"clicks":1'
check "archive forum refuses topics" "$(post_form --data-urlencode "csrf=$T" --data-urlencode 'title=Should not work' --data-urlencode 'body=Some text for the body' $B/forum/$AF/new)" 404
check "forum rules set" "$(adm -X PATCH -d '{"approve_mode":"links","require_tag":true,"counts_posts":false}' $B/api/v1/admin/nodes/2)" 204
settle 2.5
check "required tag enforced" "$(post_form --data-urlencode "csrf=$T" --data-urlencode 'title=No tag on this topic' --data-urlencode 'body=Some text for the body' $B/forum/2/new)" 422
adm -X PATCH -d '{"require_tag":false}' $B/api/v1/admin/nodes/2 >/dev/null
settle 2.5
WT=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode 'title=About giraffes today' --data-urlencode 'body=Some text about okapi friends' $B/forum/2/new)
sleep 0.5
# Staff skip the word screen, so a member's post exercises it.
curl -s -c $K -b $K -o /dev/null $B/register; T2=$(csrf $K)
reg wordy >/dev/null
W=$(mktemp); curl -s -c $W -b $W -o /dev/null $B/login; TW=$(csrf $W)
curl -s -o /dev/null -c $W -b $W --data-urlencode "csrf=$TW" --data-urlencode 'login=wordy' --data-urlencode 'password=another long passphrase' $B/login
adm -X PATCH -d '{"approve_mode":"none"}' $B/api/v1/admin/nodes/2 >/dev/null
settle 2.5
WU=$(curl -s -b $W -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$TW" --data-urlencode 'title=Giraffes and okapi' --data-urlencode 'body=Some text about okapi and giraffes here' $B/forum/2/new)
if [ -n "$WU" ]; then
  WP=$(curl -s "$WU")
  has "watched word links" "$WP" 'href="https://okapi.example/"'
  has "watched word tags" "$WP" 'href="/tag/animals"'
  adm -X PATCH -d '{"approve_mode":"links"}' $B/api/v1/admin/nodes/2 >/dev/null
  settle 2.5
  check "links held by forum rule" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" --data-urlencode 'title=Topic with a link here' --data-urlencode 'body=See https://a.example/x for more' $B/forum/2/new)" 202
  adm -X PATCH -d '{"approve_mode":"none"}' $B/api/v1/admin/nodes/2 >/dev/null
fi
check "pin with options" "$(post_form --data-urlencode "csrf=$T" -d 'days=3&global=1' $B/topic/$TID/pin)" 303
settle 2.5
LP=$(curl -s $B/latest)
has "pinned globally first" "$(grep -oE 'href="/topic/[0-9]+' <<<"$LP" | head -1)" "/topic/$TID"
check "review filter" "$(curl -s -o /dev/null -w '%{http_code}' -b $J "$B/review?kind=report&sort=age")" 200
RID=$(curl -s -b $J $B/review | grep -oE 'action="/review/[0-9]+/claim"' | head -1 | grep -oE '[0-9]+')
if [ -n "$RID" ]; then
  check "claim review" "$(post_form --data-urlencode "csrf=$T" $B/review/$RID/claim)" 303
  has "claim shown" "$(curl -s -b $J $B/review)" 'Claimed by'
fi
check "mute a tag" "$(post_form --data-urlencode "csrf=$T" --data-urlencode 'tags=stripes' $B/settings/muted-tags)" 303
no "muted tag gone from latest" "$(curl -s -b $J $B/latest)" 'Tagged topic about zebras'
post_form --data-urlencode "csrf=$T" --data-urlencode 'tags=' $B/settings/muted-tags >/dev/null
has "session shows device" "$(curl -s -A 'Mozilla/5.0 (Windows NT 10.0) Chrome/120.0' -b $J $B/settings)" 'on '
check "task with retention" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/maintenance/housekeeping)" 200

# --- quotes of deleted posts
T=$(csrf $J)
QT=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode 'title=Quoted post check' --data-urlencode 'body=Original words to quote' --data-urlencode 'tags=' $B/forum/2/new)
QI=$(grep -oE '/topic/[0-9]+' <<<"$QT" | cut -d/ -f3)
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" --data-urlencode 'body=A reply that will be deleted soon' $B/topic/$QI/reply
QP=$(curl -s -b $J "$QT" | grep -oE 'data-post="[0-9]+"' | sed -n 2p | grep -oE '[0-9]+')
check "nothing deleted yet" "$(curl -s "$B/api/v1/quotes/deleted?refs=$QI:2")" "[]"
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" $B/post/$QP/delete
has "deleted quote reported" "$(curl -s "$B/api/v1/quotes/deleted?refs=$QI:2,$QI:1,junk")" "$QI:2"
no "live quote not reported" "$(curl -s "$B/api/v1/quotes/deleted?refs=$QI:1")" "$QI:1"

# --- ASN bans, group pages, BBCode setting, delayed email
T=$(csrf $J)
has "group mention link" "$(curl -s -o /dev/null -w '%{redirect_url}' $B/u/moderators)" '/group/moderators'
check "group page" "$(curl -s -o /dev/null -w '%{http_code}' $B/group/moderators)" 200
check "unknown group" "$(curl -s -o /dev/null -w '%{http_code}' $B/group/nope)" 404
if [ -n "${BF_ASN_HEADER:-}" ]; then
  check "asn ban added" "$(adm -d '{"kind":"asn","value":"64500"}' $B/api/v1/admin/bans)" 201
  check "asn banned write" "$(curl -s -o /dev/null -w '%{http_code}' -H "$BF_ASN_HEADER: 64500" -d '{}' $B/api/v1/csp-report)" 403
  check "other asn fine" "$(curl -s -o /dev/null -w '%{http_code}' -H "$BF_ASN_HEADER: 64501" -d '{}' $B/api/v1/csp-report)" 204
  check "asn banned read still works" "$(curl -s -o /dev/null -w '%{http_code}' -H "$BF_ASN_HEADER: 64500" $B/search)" 200
fi
setting bbcode_enabled false; settle 1.5
no "bbcode can be turned off" "$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"raw":"[b]x[/b]"}' $B/api/v1/preview)" '<strong>'
setting bbcode_enabled true; settle 1.5
has "bbcode back on" "$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"raw":"[b]x[/b]"}' $B/api/v1/preview)" '<strong>'

# --- streaming, thumbnail formats, session country
has "suggestions still arrive" "$(curl -s -b $J $B/topic/$QI-quoted-post-check/)" 'Suggested topics'
no "no marker left behind" "$(curl -s -b $J $B/topic/$QI-quoted-post-check/)" 'bf-suggested'
has "member page is streamed" "$(curl -s -D - -o /dev/null -b $J $B/topic/$QI-quoted-post-check/)" 'chunked'
CT() { curl -s -o /dev/null -w '%{content_type}' -H "Accept: $1" "$B$BIG-960.png"; }
check "avif thumbnail" "$(CT 'image/avif,image/webp,*/*')" 'image/avif'
check "webp thumbnail" "$(CT 'image/webp,*/*')" 'image/webp'
check "png thumbnail" "$(CT '*/*')" 'image/png'
has "thumbnails vary by accept" "$(curl -s -D - -o /dev/null $B$BIG-960.png)" 'Accept'

# --- strict CSP, image hosts, session country
HH=$(curl -s -D - -o /dev/null $B/search)
no "no inline styles allowed" "$(grep -i '^content-security-policy' <<<"$HH" | grep -o "style-src [^;]*")" 'unsafe-inline'
setting csp_image_hosts '"https://img.example"'; settle 1.5
has "image hosts restricted" "$(curl -s -D - -o /dev/null $B/search)" "https://i.ytimg.com https://img.example data:"
setting csp_image_hosts '""'
if [ -n "${BF_GEO_HEADER:-}" ]; then
  G=$(mktemp); curl -s -c $G -b $G -o /dev/null $B/login
  curl -s -o /dev/null -c $G -b $G --data-urlencode "csrf=$(csrf $G)" --data-urlencode 'login=wordy' --data-urlencode 'password=another long passphrase' $B/login
  has "session shows country" "$(curl -s -b $G -H "$BF_GEO_HEADER: PL" $B/settings)" '· PL'
  rm -f $G
fi

# --- API keys, webhooks, site text, impersonation, topic templates
T=$(csrf $J)
apikey() { curl -s -b $1 -H "X-CSRF-Token: $2" -H 'Content-Type: application/json' -d "{\"name\":\"$3\",\"scope\":\"$4\"}" $B/api/v1/me/api-keys | grep -oE 'bf_[0-9a-f]+'; }
KR=$(apikey $J $T reader read); KW=$(apikey $J $T writer write); KA=$(apikey $J $T boss admin)
has "key authenticates" "$(curl -s -H "Authorization: Bearer $KR" $B/api/v1/me)" '"scope":"read"'
check "bad key refused" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer bf_nope' $B/api/v1/me)" 403
check "read key cannot write" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $KR" --data-urlencode 'body=Written by an API key' $B/topic/$TID/reply)" 403
check "write key writes without csrf" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $KW" --data-urlencode 'csrf=x' --data-urlencode 'body=Written by an API key' $B/topic/$TID/reply)" 303
check "write key is not admin" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $KW" $B/api/v1/admin/stats)" 403
check "admin key is admin" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $KA" $B/api/v1/admin/stats)" 200
check "keys cannot mint keys" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $KA" -H 'Content-Type: application/json' -d '{"name":"x","scope":"admin"}' $B/api/v1/me/api-keys)" 403
no "key list hides tokens" "$(curl -s -b $J $B/api/v1/me/api-keys)" 'bf_'
if [ -n "${OIDC:-}" ] && [ "${BF_WEBHOOK_ALLOW_LOOPBACK:-}" = 1 ]; then
  check "webhook needs known events" "$(adm -d '{"url":"https://a.example/","events":["nope"]}' $B/api/v1/admin/webhooks)" 422
  check "webhook refuses bad scheme" "$(adm -d '{"url":"file:///etc/passwd","events":["*"]}' $B/api/v1/admin/webhooks)" 422
  HS=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d "{\"url\":\"$OIDC/hook\",\"events\":[\"post.created\"]}" $B/api/v1/admin/webhooks | grep -oE '"secret":"[0-9a-f]+' | cut -d'"' -f4)
  adm -d "{\"url\":\"$OIDC/hook-fail\",\"events\":[\"*\"]}" $B/api/v1/admin/webhooks >/dev/null
  curl -s -o /dev/null -H "Authorization: Bearer $KW" --data-urlencode 'csrf=x' --data-urlencode 'body=This reply fires the hook' $B/topic/$TID/reply
  settle 5
  node -e '
    const [log, secret] = [JSON.parse(process.argv[1]), process.argv[2]];
    const h = log.find((x) => x.path === "/hook" && x.body.includes("fires") === false || x.path === "/hook");
    const want = "sha256=" + require("crypto").createHmac("sha256", secret).update(h.body).digest("hex");
    console.log(h.event === "post.created" && h.signature === want ? "ok   webhook delivered and signed" : "FAIL webhook signature " + JSON.stringify(h));
  ' "$(curl -s $OIDC/hook-log)" "$HS" | tee /tmp/hook.txt; grep -q '^ok' /tmp/hook.txt || fail=1
  has "failed delivery retries" "$(curl -s -b $J $B/api/v1/admin/webhook-deliveries)" '"last_status":500'
fi
check "unknown text refused" "$(adm -X PUT -d '{"value":"x"}' $B/api/v1/admin/text/en/no.such.key)" 422
check "text override" "$(adm -X PUT -d '{"value":"Boards"}' $B/api/v1/admin/text/en/nav.forums)" 204
has "override shown" "$(curl -s -H 'Accept-Language: en' $B/search)" '>Boards</a>'
check "override removed" "$(adm -X DELETE $B/api/v1/admin/text/en/nav.forums)" 204
no "default back" "$(curl -s -H 'Accept-Language: en' $B/search)" '>Boards</a>'
check "cannot impersonate admins" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/users/1/impersonate)" 422
WID=$(curl -s -b $J "$B/api/v1/admin/users?q=wordy" | grep -oE '"id":[0-9]+' | head -1 | cut -d: -f2)
I=$(mktemp); cp $J $I
check "impersonate" "$(curl -s -o /dev/null -w '%{http_code}' -c $I -b $I -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/users/$WID/impersonate)" 204
IP=$(curl -s -c $I -b $I $B/search)
has "banner shown" "$IP" 'You are viewing the site as wordy'
check "no admin while impersonating" "$(curl -s -o /dev/null -w '%{http_code}' -b $I $B/api/v1/admin/stats)" 403
has "impersonation logged" "$(curl -s -b $J $B/api/v1/admin/audit)" 'user.impersonate'
check "stop impersonating" "$(curl -s -o /dev/null -w '%{http_code}' -c $I -b $I --data-urlencode "csrf=$(csrf $I)" $B/impersonate/stop)" 303
no "banner gone" "$(curl -s -b $I $B/search)" 'You are viewing'
check "admin again" "$(curl -s -o /dev/null -w '%{http_code}' -b $I $B/api/v1/admin/stats)" 200
rm -f $I
check "set template" "$(adm -X PATCH -d '{"template_md":"Steps to reproduce:"}' $B/api/v1/admin/nodes/2)" 204
settle 2.5
has "template prefilled" "$(curl -s -b $J $B/forum/2/new)" 'Steps to reproduce:'

# --- API key form, OAuth2 server, SSO, notices, sidebar
T=$(csrf $J)
has "key form shows token once" "$(curl -s -b $J --data-urlencode "csrf=$T" -d 'name=fromform&scope=read' $B/settings/api-keys)" 'bf_'
has "key listed in settings" "$(curl -s -b $J $B/settings)" 'fromform'
OC=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"name":"Test App","redirect_uris":["https://app.example/cb"]}' $B/api/v1/admin/oauth-clients)
CID=$(node -e 'console.log(JSON.parse(process.argv[1]).client_id)' "$OC"); CSEC=$(node -e 'console.log(JSON.parse(process.argv[1]).client_secret)' "$OC")
VER=verifier-verifier-verifier-verifier-verifier-1234
CHAL=$(node -e 'console.log(require("crypto").createHash("sha256").update(process.argv[1]).digest("base64url"))' "$VER")
AQ="response_type=code&client_id=$CID&scope=read&state=st8&code_challenge=$CHAL&code_challenge_method=S256"
check "unregistered return address refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J "$B/oauth/authorize?$AQ&redirect_uri=https://evil.example/")" 400
has "consent page" "$(curl -s -b $J "$B/oauth/authorize?$AQ&redirect_uri=https://app.example/cb")" 'Allow Test App?'
check "guests go to login" "$(curl -s -o /dev/null -w '%{redirect_url}' "$B/oauth/authorize?$AQ&redirect_uri=https://app.example/cb")" "$B/login"
AP() { curl -s -o /dev/null -w '%{redirect_url}' -b $J --data-urlencode "csrf=$T" -d "client_id=$CID&redirect_uri=https://app.example/cb&scope=read&state=st8&code_challenge=$CHAL" "$@" $B/oauth/authorize; }
has "deny returns error" "$(AP -d deny=1)" 'error=access_denied'
BACK=$(AP); has "approve returns state" "$BACK" 'state=st8'
CODE=$(grep -oE 'code=[0-9a-f]+' <<<"$BACK" | cut -d= -f2)
TK() { curl -s -d "grant_type=authorization_code&code=$CODE&redirect_uri=https://app.example/cb&client_id=$CID&client_secret=$CSEC&code_verifier=$1" $B/oauth/token; }
has "wrong verifier refused" "$(TK wrong)" 'invalid_grant'
has "failed attempt burns the code" "$(TK $VER)" 'invalid_grant'
BACK=$(AP); CODE=$(grep -oE 'code=[0-9a-f]+' <<<"$BACK" | cut -d= -f2)
has "bad secret refused" "$(curl -s -d "grant_type=authorization_code&code=$CODE&redirect_uri=https://app.example/cb&client_id=$CID&client_secret=nope&code_verifier=$VER" $B/oauth/token)" 'invalid_client'
TOK=$(TK $VER | grep -oE 'bf_[0-9a-f]+')
has "token works on userinfo" "$(curl -s -H "Authorization: Bearer $TOK" $B/oauth/userinfo)" 'preferred_username'
has "code is single use" "$(TK $VER)" 'invalid_grant'
check "app tokens are not admin" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOK" $B/api/v1/admin/stats)" 403
check "read token cannot write" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOK" --data-urlencode 'body=No writing allowed here' $B/topic/$TID/reply)" 403
check "deleting the app revokes" "$(adm -X DELETE $B/api/v1/admin/oauth-clients/$CID)" 204
check "revoked token refused" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOK" $B/oauth/userinfo)" 403
if [ -n "${BF_SSO_SECRET:-}" ]; then
  has "sso on login page" "$(curl -s $B/login)" 'action="/auth/sso"'
  S1=$(mktemp)
  check "sso signs in" "$(curl -s -L -o /dev/null -w '%{url_effective}' -c $S1 -b $S1 $B/auth/sso)" "$B/"
  has "sso account made" "$(curl -s -b $S1 $B/settings)" 'sso.user@example.com'
  check "forged sso refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $S1 "$B/auth/sso/callback?sso=AAAA&sig=00")" 400
  rm -f $S1
fi
check "notices readable" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/api/v1/admin/notices)" 200
setting sidebar_links '"[{\"label\":\"Rules\",\"url\":\"/rules-link\"},{\"label\":\"Bad\",\"url\":\"javascript:x\"}]"'; settle 1.5
SB=$(curl -s $B/search)
has "sidebar" "$SB" 'href="/rules-link"'
no "sidebar drops unsafe links" "$SB" 'javascript:x'
setting sidebar_links '""'

# --- themes, safe mode, widgets
T=$(csrf $J)
check "bad token refused" "$(adm -d '{"slug":"bad","name":"Bad","tokens":{"light":{"accent":"red; } body { display:none"}}}' $B/api/v1/admin/themes)" 422
check "unknown slot refused" "$(adm -d '{"slug":"bad","name":"Bad","slots":{"nope":"x"}}' $B/api/v1/admin/themes)" 422
TH=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"slug":"sunny","name":"Sunny","tokens":{"light":{"accent":"#ff8800"}},"css":".sunny{color:red}","slots":{"footer":"<p class=\"sunny-foot\">{{ site_name }}</p><script>alert(1)</script>"}}' $B/api/v1/admin/themes | grep -oE '[0-9]+' | head -1)
check "theme not forced on anyone" "$(curl -s $B/search | grep -c '/theme/')" 0
check "preview just for me" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -c $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/themes/$TH/preview)" 204
PV=$(curl -s -b $J $B/search)
has "preview shows theme" "$PV" 'sunny-foot'
no "slot output sanitized" "$PV" 'alert(1)'
check "guests still plain" "$(curl -s $B/search | grep -c 'sunny-foot')" 0
curl -s -o /dev/null -b $J -c $J -X POST -H "X-CSRF-Token: $T" $B/api/v1/admin/themes/preview/stop
setting default_theme_id $TH; settle 1.5
curl -s -o /dev/null -b $J -X PATCH -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"selectable":true}' $B/api/v1/admin/themes/$TH
GP=$(curl -s $B/search)
has "default theme for guests" "$GP" 'sunny-foot'
CSSU=$(grep -oE '/theme/[0-9a-f]+\.css' <<<"$GP" | head -1)
has "theme css served" "$(curl -s $B$CSSU)" '--bf-accent:#f80'
has "theme css immutable" "$(curl -s -D - -o /dev/null $B$CSSU)" 'immutable'
check "safe mode for admins only" "$(curl -s -o /dev/null -w '%{http_code}' $B/safe-mode)" 403
S2=$(mktemp); cp $J $S2
curl -s -o /dev/null -b $S2 -c $S2 $B/safe-mode
no "safe mode hides the theme" "$(curl -s -b $S2 $B/search)" 'sunny-foot'
rm -f $S2
setting default_theme_id 0; settle 1.5
check "member picks theme" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d "theme=$TH" $B/settings/theme)" 303
has "member sees chosen theme" "$(curl -s -b $W $B/search)" 'sunny-foot'
curl -s -o /dev/null -b $W --data-urlencode "csrf=$TW" -d 'theme=' $B/settings/theme
check "forum theme" "$(adm -X PUT -d "{\"theme\":$TH}" $B/api/v1/admin/nodes/2/theme)" 204
has "forum override" "$(curl -s $B/forum/2-introductions/)" 'sunny-foot'
adm -X PUT -d '{"theme":null}' $B/api/v1/admin/nodes/2/theme >/dev/null
ZD=$(mktemp -d)
node -e '
const zlib=require("zlib"),fs=require("fs");
const files={"theme.toml":"id = \"zipped\"\nname = \"Zipped\"\nversion = \"3.1.0\"\ntype = \"component\"","styles/a.css":".zipped-comp{color:red}"};
const crcT=[...Array(256)].map((_,n)=>{let c=n;for(let k=0;k<8;k++)c=c&1?0xedb88320^(c>>>1):c>>>1;return c>>>0});
const crc=(b)=>{let c=~0;for(const x of b)c=crcT[(c^x)&255]^(c>>>8);return ~c>>>0};
const parts=[],cd=[];let off=0;
for(const [n,t] of Object.entries(files)){const d=Buffer.from(t),nb=Buffer.from(n),h=Buffer.alloc(30);h.writeUInt32LE(0x04034b50,0);h.writeUInt16LE(20,4);h.writeUInt32LE(crc(d),14);h.writeUInt32LE(d.length,18);h.writeUInt32LE(d.length,22);h.writeUInt16LE(nb.length,26);parts.push(h,nb,d);
const c=Buffer.alloc(46);c.writeUInt32LE(0x02014b50,0);c.writeUInt16LE(20,4);c.writeUInt16LE(20,6);c.writeUInt32LE(crc(d),16);c.writeUInt32LE(d.length,20);c.writeUInt32LE(d.length,24);c.writeUInt16LE(nb.length,28);c.writeUInt32LE(off,42);cd.push(c,nb);off+=30+nb.length+d.length}
const cdb=Buffer.concat(cd),e=Buffer.alloc(22);e.writeUInt32LE(0x06054b50,0);e.writeUInt16LE(2,8);e.writeUInt16LE(2,10);e.writeUInt32LE(cdb.length,12);e.writeUInt32LE(off,16);
fs.writeFileSync(process.argv[1],Buffer.concat([...parts,cdb,e]));' "$ZD/t.zip"
check "install zip" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -F "file=@$ZD/t.zip" $B/api/v1/admin/themes/install)" 201
has "zip version recorded" "$(curl -s -b $J $B/api/v1/admin/themes)" '"version":"3.1.0"'
CC=$(curl -s $B/search | grep -oE '/theme/[0-9a-f]+\.css' | head -1)
has "component applies site-wide" "$(curl -s $B$CC)" '.zipped-comp'
rm -rf $ZD
check "bad widget refused" "$(adm -d '{"area":"home_top","kind":"nope"}' $B/api/v1/admin/widgets)" 422
W1=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"area":"home_top","kind":"custom","title":"Welcome box","config":{"markdown":"**hello** <script>alert(2)</script>"}}' $B/api/v1/admin/widgets | grep -oE '[0-9]+' | head -1)
W2=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"area":"home_top","kind":"stats","title":"Numbers"}' $B/api/v1/admin/widgets | grep -oE '[0-9]+' | head -1)
for k in online_users latest_topics top_posters tag_cloud; do check "widget $k" "$(adm -d "{\"area\":\"home_bottom\",\"kind\":\"$k\",\"title\":\"W $k\"}" $B/api/v1/admin/widgets)" 201; done
settle 7
HP=$(curl -s $B/)
has "custom widget cooked" "$HP" '<strong>hello</strong>'
no "custom widget sanitized" "$HP" '<script>alert(2)'
has "stats widget" "$HP" 'Numbers'
has "latest widget" "$HP" 'W latest_topics'
check "arrange" "$(adm -X PUT -d "{\"area\":\"home_top\",\"ids\":[$W2,$W1]}" $B/api/v1/admin/widgets/arrange)" 204
settle 7
HP=$(curl -s $B/)
has "new order" "$(grep -oE 'Numbers|Welcome box' <<<"$HP" | head -1)" 'Numbers'
check "delete widget" "$(adm -X DELETE $B/api/v1/admin/widgets/$W1)" 204
settle 7
no "widget gone" "$(curl -s $B/)" 'Welcome box'

# --- plugin host
T=$(csrf $J)
# The fixture is unsigned, and signed plugins are required by default. This block tests
# install/enable mechanics, not signing policy, so turn the requirement off first.
check "allow unsigned plugins for this test" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -X PUT -d '{"value":false}' $B/api/v1/admin/settings/require_signed_plugins)" 204
PZ=$(mktemp -d); node "$(dirname "$0")/../support/mkzip.mjs" "$(dirname "$0")/../fixtures/plugin" "$PZ/p.zip"
check "install plugin" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -F "file=@$PZ/p.zip" $B/api/v1/admin/plugins/install)" 201
no "installed is not enabled" "$(curl -s $B/search)" 'plugin-foot'
check "plugin route off until enabled" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/p/test-plugin/x)" 404
check "capabilities must match" "$(adm -d '{"capabilities":["kv"]}' $B/api/v1/admin/plugins/test-plugin/enable)" 422
check "members cannot enable" "$(curl -s -o /dev/null -w '%{http_code}' -b $W -H "X-CSRF-Token: $TW" -H 'Content-Type: application/json' -d '{"capabilities":["kv","api_route"]}' $B/api/v1/admin/plugins/test-plugin/enable)" 403
check "approve and enable" "$(adm -d '{"capabilities":["kv","api_route"]}' $B/api/v1/admin/plugins/test-plugin/enable)" 204
PG=$(curl -s $B/search)
has "plugin slot rendered" "$PG" 'plugin-foot'
no "plugin slot sanitized" "$PG" 'alert(3)'
has "header icon slot rendered" "$PG" 'plugin-icon'
has "post slot rendered" "$(curl -s -L $B/topic/$TID-x/)" 'plugin-after-post'
JS=$(grep -oE '/plugins/test-plugin\.[0-9a-f]+\.js' <<<"$PG" | head -1)
has "plugin client served" "$(curl -s $B$JS)" 'registerIsland'
SF=$(mktemp); cp $J $SF; curl -s -o /dev/null -b $SF -c $SF $B/safe-mode
no "safe mode hides plugins" "$(curl -s -b $SF $B/search)" 'plugin-foot'
rm -f $SF
RP() { curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" --data-urlencode "body=$1" $B/topic/$TID/reply; }
check "filter rejects" "$(RP 'this has plugin-block in it')" 422
check "filter changes text" "$(RP 'please plugin-swap this text')" 303
HN=$(curl -s $B/api/v1/topics/$TID/state | grep -oE '[0-9]+')
has "plugin rewrote the post" "$(curl -s $B/api/v1/topics/$TID/posts/$HN)" 'changed by plugin'
check "normal post fine" "$(RP 'An ordinary reply goes through')" 303
sleep 1
has "action ran, kv stored, api route answers" "$(curl -s $B/api/p/test-plugin/status)" '"kv":"seen"'
check "plugin post needs csrf" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -X POST -d '{}' $B/api/p/test-plugin/x)" 400
check "plugin settings" "$(adm -X PUT -d '{"a":1}' $B/api/v1/admin/plugins/test-plugin/settings)" 204
check "bad settings" "$(adm -X PUT -d '[1]' $B/api/v1/admin/plugins/test-plugin/settings)" 422
check "endless plugin stopped, post goes on" "$(RP 'plugin-spin forever here')" 303
for i in 1 2 3 4; do RP "plugin-trap number $i here" >/dev/null; done
has "keeps failing, gets disabled" "$(curl -s -b $J $B/api/v1/admin/plugins)" '"enabled":false'
has "admins told" "$(curl -s -b $J $B/api/v1/admin/notices)" 'kept failing'
check "disabled plugin gone" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/p/test-plugin/x)" 404
check "uninstall" "$(adm -X DELETE $B/api/v1/admin/plugins/test-plugin)" 204
rm -rf $PZ

# --- schemes, partials, member links, events, URL installs
T=$(csrf $J)
TH2=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"slug":"schemed","name":"Schemed","tokens":{"schemes":{"sepia":{"bg":"#f4ecd8"}}},"slots":{"partial:post_author":"<em class=\"pa\">{{ author }}</em>","partial:topic_title":"<a class=\"tt\" href=\"{{ url }}\">{{ title }}</a>"}}' $B/api/v1/admin/themes | grep -oE '[0-9]+' | head -1)
check "unknown partial refused" "$(adm -d '{"slug":"x1","name":"X","slots":{"partial:nope":"x"}}' $B/api/v1/admin/themes)" 422
check "bad scheme token refused" "$(adm -d '{"slug":"x2","name":"X","tokens":{"schemes":{"a b":{"bg":"#fff"}}}}' $B/api/v1/admin/themes)" 422
check "choose scheme and links" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d "theme=$TH2&scheme=sepia" --data-urlencode 'sidebar=[{"label":"Mine","url":"/my-link"},{"label":"Bad","url":"javascript:y"}]' $B/settings/theme)" 303
MP=$(curl -s -b $W $B/topic/$TID-x/)
MP=$(curl -s -b $W -L $B/topic/$TID-x/)
has "scheme on page" "$MP" 'data-scheme="sepia"'
has "partial replaces author" "$MP" 'class="pa"'
has "member sidebar link" "$MP" 'href="/my-link"'
no "unsafe member link dropped" "$MP" 'javascript:y'
has "scheme css" "$(curl -s $B$(grep -oE '/theme/[0-9a-f]+\.css' <<<"$MP" | head -1))" 'data-scheme=sepia'
has "partial replaces title" "$(curl -s -b $W $B/latest)" 'class="tt"'
curl -s -o /dev/null -b $W --data-urlencode "csrf=$TW" -d 'theme=' $B/settings/theme
EV=$(curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' -d '{"area":"home_bottom","kind":"upcoming_events","title":"Soon","config":{"events":[{"date":"2999-01-01","title":"Far future <b>"},{"date":"2000-01-01","title":"Long gone"}]}}' $B/api/v1/admin/widgets | grep -oE '[0-9]+' | head -1)
settle 7
HP=$(curl -s $B/)
has "event listed, escaped" "$HP" 'Far future &lt;b&gt;'
no "past event dropped" "$HP" 'Long gone'
has "edit layout offered to admins" "$(curl -s -b $J $B/)" 'edit_layout=1'
no "not to guests" "$HP" 'edit_layout=1'
check "install from internal address refused" "$(adm -d '{"url":"https://127.0.0.1:1/x.zip"}' $B/api/v1/admin/plugins/install-url)" 422
check "theme url needs https" "$(adm -d '{"url":"ftp://a.example/x.zip"}' $B/api/v1/admin/themes/install-url)" 422
setting require_signed_plugins true; settle 1.5
PZ=$(mktemp -d); node "$(dirname "$0")/../support/mkzip.mjs" "$(dirname "$0")/../fixtures/plugin" "$PZ/p.zip"
check "unsigned refused when required" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -F "file=@$PZ/p.zip" $B/api/v1/admin/plugins/install)" 422
setting require_signed_plugins false; rm -rf $PZ

adm_body() { curl -s -b $J -H "X-CSRF-Token: $T" -H 'Content-Type: application/json' "$@"; }
# --- plugin platform: tables, content, jobs, notifications, pages, widgets, sitemap
T=$(csrf $J)
PZ=$(mktemp -d); node "$(dirname "$0")/../support/mkzip.mjs" "$(dirname "$0")/../fixtures/plugin-ext" "$PZ/p.zip"
check "install extension plugin" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -F "file=@$PZ/p.zip" $B/api/v1/admin/plugins/install)" 201
check "enable it" "$(adm -d '{"capabilities":["tables","content.read","content.write","jobs","notify","api_route","kv","http:127.0.0.1","login"]}' $B/api/v1/admin/plugins/ext-plugin/enable)" 204
PA() { curl -s $B/api/p/ext-plugin/$1; }
has "table insert" "$(PA db-insert)" '"id":1'
has "table select" "$(PA db-select)" '"body":"hello"'
has "core tables out of reach" "$(PA db-users)" 'Unknown table users'
has "content read" "$(PA read)" '"title":"Best casino in town"'
has "unreadable topic refused" "$(PA read-missing)" 'No such topic'
check "member watches the topic" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'level=watch' $B/topic/2/follow)" 303
check "member wants reply emails" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'reply_app=on&reply_email=on' $B/settings/notifications)" 303
setting email_delay_minutes 0; settle 1.5
LIVE=$(mktemp); curl -s -N --max-time 4 "$B/api/v1/live?channels=item:2" > $LIVE &
LP=$!; sleep 1
has "reply as own account" "$(PA reply)" '"number":'
wait $LP
has "live update for bot post" "$(cat $LIVE)" 'plugin-ext-plugin'
rm -f $LIVE
if [ -n "$MAIL" ]; then
  for _ in $(seq 20); do grep -q 'plugin-ext-plugin posted' "$MAIL"/*.eml 2>/dev/null && break; sleep 0.5; done
  check "email for bot post" "$(cat "$MAIL"/*.eml | grep -c 'plugin-ext-plugin posted' | sed 's/^[1-9][0-9]*$/sent/')" sent
fi
setting plugin_posts_per_hour 0; settle 1.5
has "post rate limit" "$(PA reply)" 'posted too much'
setting plugin_posts_per_hour 60; settle 1.5
has "watchers hear about bot posts" "$(curl -s -b $W $B/notifications)" 'plugin-ext-plugin posted in'

has "bot post shown" "$(curl -s -L $B/topic/2-x/)" 'plugin-ext-plugin'
no "bot is not an admin" "$(curl -s -b $J "$B/api/v1/admin/users?q=plugin-ext")" '"primary_group_id":4'
setting plugin_storage_mb 0; settle 1.5
has "storage limit stops inserts" "$(PA db-insert)" 'used all its storage'
check "key-value limit" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/p/ext-plugin/kv-set)" 403
has "reads still work at the limit" "$(PA db-select)" '"body":"hello"'
setting plugin_storage_mb 256; settle 1.5
has "writes resume under the limit" "$(PA db-insert)" '"id":'
check "key-value resumes" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/p/ext-plugin/kv-set)" 200
if [ -n "${OIDC:-}" ]; then
  has "approved host fetched" "$(PA http-get)" '"path":"/hook"'
  check "other host refused" "$(curl -s -o /dev/null -w '%{http_code}' $B/api/p/ext-plugin/http-other)" 403
  has "webhook for bot post" "$(settle 6; curl -s $OIDC/hook-log)" 'plugin-ext-plugin'
fi
has "job queued" "$(PA job)" '"queued":true'
settle 7
has "job ran" "$(PA db-select)" 'from-job'
has "plugin notifies" "$(PA notify)" '"sent":1'
has "notification shown" "$(curl -s -b $J $B/notifications)" 'Plugin says hi'
has "off-site notification url refused" "$(PA notify-evil)" 'path on this site'
check "after-cook filter on post" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" --data-urlencode "body=please ext-cook this post" $B/topic/2/reply)" 303
ACP=$(curl -s -L $B/topic/2-x/)
has "after-cook html stored" "$ACP" 'cooked</p>'
no "after-cook html sanitized" "$ACP" 'alert(5)'
has "settings link" "$(curl -s -b $J $B/settings)" 'href="/plugin/ext-plugin/prefs"'
has "modcp link" "$(curl -s -b $J $B/modcp)" 'href="/plugin/ext-plugin/mod"'
has "profile tab" "$(curl -s -L $B/profile/1-x/)" 'href="/plugin/ext-plugin/tab?user=1"'
has "panel renders" "$(curl -s -b $J $B/plugin/ext-plugin/prefs)" 'ext-panel'
no "panel sanitized" "$(curl -s -b $J $B/plugin/ext-plugin/prefs)" 'alert(6)'
check "settings panel needs an account" "$(curl -s -o /dev/null -w '%{http_code}' $B/plugin/ext-plugin/prefs)" 403
check "modcp panel closed to guests" "$(curl -s -o /dev/null -w '%{http_code}' $B/plugin/ext-plugin/mod)" 403
check "modcp panel closed to members" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/plugin/ext-plugin/mod)" 403
check "admin panel closed to members" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/plugin/ext-plugin/admin)" 403
check "admin panel opens for admins" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/plugin/ext-plugin/admin)" 200
check "unknown panel 404" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/plugin/ext-plugin/nope)" 404
check "plugin widget" "$(adm -d '{"area":"home_bottom","kind":"plugin","title":"Ext","config":{"plugin":"ext-plugin","key":"w"}}' $B/api/v1/admin/widgets)" 201
settle 7
HP=$(curl -s $B/)
has "plugin widget renders" "$HP" 'ext-widget'
no "plugin widget sanitized" "$HP" 'alert(7)'
SM=$(curl -s $B/sitemap.xml)
has "sitemap source" "$SM" '/ext-page'
no "off-site sitemap path dropped" "$SM" 'evil.test'
has "serializer fields under the plugin's name" "$(curl -s $B/api/v1/topics/2/state)" '"ext-plugin":{"score":42}'
has "core fields kept" "$(curl -s $B/api/v1/topics/2/state)" '"highest_post_number"'
has "member filter applied" "$(adm_body -d '{"filter":"x","group":2}' $B/api/v1/admin/plugins/ext-plugin/member-filter)" '"matched":2'
check "member filter cannot grant staff" "$(adm -d '{"filter":"x","group":4}' $B/api/v1/admin/plugins/ext-plugin/member-filter)" 422
has "content page is in the menu" "$(curl -s $B/search)" 'Articles'
check "content page is public" "$(curl -s -o /dev/null -w '%{http_code}' $B/plugin/ext-plugin/articles)" 200
no "content page is indexable" "$(curl -s $B/plugin/ext-plugin/articles)" 'name="robots" content="noindex"'
has "login option offered" "$(curl -s $B/login)" 'Sign in with Test IdP'
LJ=$(mktemp)
has "login begins at the provider" "$(curl -s -D - -o /dev/null -c $LJ -b $LJ $B/auth/p/ext-plugin/idp)" 'https://idp.example/start'
check "forged login state refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $LJ "$B/auth/p/ext-plugin/idp/callback?state=forged")" 400
check "no login state refused" "$(curl -s -o /dev/null -w '%{http_code}' "$B/auth/p/ext-plugin/idp/callback?state=")" 400
LS=$(grep -oE 'bf_plugin_login\s+[0-9a-f]{64}' $LJ | awk '{print $2}')
check "login completes" "$(curl -s -o /dev/null -w '%{http_code}' -c $LJ -b $LJ "$B/auth/p/ext-plugin/idp/callback?state=$LS")" 303
has "signed in by the plugin" "$(curl -s -b $LJ $B/settings)" 'extlogin@example.com'
rm -f $LJ
check "uninstall extension plugin" "$(adm -X DELETE $B/api/v1/admin/plugins/ext-plugin)" 204
check "panel gone" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/plugin/ext-plugin/prefs)" 404
check "reinstall" "$(curl -s -o /dev/null -w '%{http_code}' -b $J -H "X-CSRF-Token: $T" -F "file=@$PZ/p.zip" $B/api/v1/admin/plugins/install)" 201
adm -d '{"capabilities":["tables","content.read","content.write","jobs","notify","api_route","kv","http:127.0.0.1","login"]}' $B/api/v1/admin/plugins/ext-plugin/enable >/dev/null
no "uninstall dropped its tables" "$(PA db-select)" 'hello'
adm -X DELETE $B/api/v1/admin/plugins/ext-plugin >/dev/null
rm -rf $PZ

# --- private messages
T=$(csrf $J)
ADMINN=$(curl -s -b $J $B/ | grep -oE 'class="nav-user" href="/settings">[^<]+' | sed 's/.*>//')
reg pmoutsider >/dev/null; K=$(mktemp); curl -s -c $K -b $K -o /dev/null $B/login; T2=$(csrf $K)
curl -s -o /dev/null -c $K -b $K --data-urlencode "csrf=$T2" --data-urlencode 'login=pmoutsider' --data-urlencode 'password=another long passphrase' $B/login
PMS() { curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" "$@"; }
check "pm needs csrf" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=" --data-urlencode "to=$ADMINN" -d 'subject=x&body=y' $B/messages)" 400
check "guests have no inbox" "$(curl -s -o /dev/null -w '%{http_code}' $B/messages)" 403
check "unknown recipient refused" "$(PMS --data-urlencode 'to=nobody-here' -d 'subject=Hi&body=Hello' $B/messages)" 422
check "cannot message only yourself" "$(PMS --data-urlencode 'to=wordy' -d 'subject=Hi&body=Hello' $B/messages)" 422
PMU=$(curl -s -o /dev/null -w '%{redirect_url}' -b $W --data-urlencode "csrf=$TW" --data-urlencode "to=$ADMINN" --data-urlencode 'subject=Secret plans' --data-urlencode 'body=The **zebra** code word <script>alert(9)</script>' $B/messages)
has "conversation created" "$PMU" '/messages/'
PM=$(grep -oE '[0-9]+$' <<<"$PMU")
has "recipient notified" "$(curl -s -b $J $B/notifications)" 'wordy sent you a message: Secret plans'
has "unread in inbox" "$(curl -s -b $J $B/messages)" '<strong>Secret plans</strong>'
PV=$(curl -s -b $J $B/messages/$PM)
has "recipient reads" "$PV" '<strong>zebra</strong>'
no "message sanitized" "$PV" '<script>alert(9)'
no "read clears unread" "$(curl -s -b $J $B/messages)" '<strong>Secret plans</strong>'
has "read receipt" "$(curl -s -b $W $B/messages/$PM)" 'has read the latest message'
check "outsider gets 404" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/messages/$PM)" 404
check "outsider cannot reply" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" -d 'body=sneaky' $B/messages/$PM/reply)" 404
check "guest cannot read" "$(curl -s -o /dev/null -w '%{http_code}' $B/messages/$PM)" 403
no "public search cannot find it" "$(curl -s -b $J "$B/search?q=zebra")" 'Secret plans'
has "own search finds it" "$(curl -s -b $W "$B/messages?q=zebra")" 'Secret plans'
no "outsider search cannot find it" "$(curl -s -b $K "$B/messages?q=zebra")" 'Secret plans'
check "recipient replies" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'body=Understood' $B/messages/$PM/reply)" 303
check "only the starter adds people" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'username=pmoutsider' $B/messages/$PM/add)" 403
check "starter adds someone" "$(PMS -d 'username=pmoutsider' $B/messages/$PM/add)" 303
has "added person reads history" "$(curl -s -b $K $B/messages/$PM)" 'zebra'
check "starter removes them" "$(PMS -d 'username=pmoutsider' $B/messages/$PM/remove)" 303
check "removed person loses access" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/messages/$PM)" 404
check "archive" "$(PMS -d 'archived=1' $B/messages/$PM/archive)" 303
no "archived leaves inbox" "$(curl -s -b $W $B/messages)" 'Secret plans'
has "archive lists it" "$(curl -s -b $W "$B/messages?folder=archive")" 'Secret plans'
check "reply brings it back" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'body=One more thing' $B/messages/$PM/reply)" 303
has "back in inbox" "$(curl -s -b $W $B/messages)" 'Secret plans'
check "mark unread" "$(PMS $B/messages/$PM/unread)" 303
has "shows unread" "$(curl -s -b $W $B/messages)" '<strong>Secret plans</strong>'
check "outsider ignores wordy" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" -d 'on=1' $B/profile/$(curl -s -b $J "$B/api/v1/admin/users?q=wordy" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')/ignore)" 303
check "ignored sender refused" "$(PMS --data-urlencode 'to=pmoutsider' -d 'subject=Hello&body=Hello there' $B/messages)" 422
check "leave" "$(PMS $B/messages/$PM/leave)" 303
check "left means no access" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/messages/$PM)" 404

# --- reactions, reputation
RT=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode 'title=Depth feature topic' --data-urlencode 'body=Some question worth answering here' $B/forum/2/new)
RTID=$(grep -oE '/topic/[0-9]+' <<<"$RT" | grep -oE '[0-9]+')
check "member answers" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'body=Here is the full answer to it' $B/topic/$RTID/reply)" 303
RP=$(curl -s -L -b $J "$RT")
RPID=$(grep -oE 'action="/post/[0-9]+/solve"' <<<"$RP" | head -1 | grep -oE '[0-9]+')
check "members cannot change reactions" "$(curl -s -o /dev/null -w '%{http_code}' -b $W -H "X-CSRF-Token: $TW" -H 'Content-Type: application/json' -d '{"name":"X","icon":"x","score":1,"enabled":true,"position":9}' $B/api/v1/admin/reactions)" 403
check "bad score refused" "$(adm -d '{"name":"X","icon":"x","score":5,"enabled":true,"position":9}' $B/api/v1/admin/reactions)" 422
check "enable funny" "$(adm -X PUT -d '{"name":"Funny","icon":"F!","score":0,"enabled":true,"position":4}' $B/api/v1/admin/reactions/4)" 204
check "enable disagree" "$(adm -X PUT -d '{"name":"Disagree","icon":"D!","score":-1,"enabled":true,"position":5}' $B/api/v1/admin/reactions/5)" 204
check "disabled reaction refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'reaction=2' $B/post/$RPID/react)" 422
check "react" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'reaction=1' $B/post/$RPID/react)" 303
has "who reacted" "$(curl -s $B/post/$RPID/reactions)" 'Like'
check "change reaction" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'reaction=5' $B/post/$RPID/react)" 303
has "one reaction per member" "$(curl -s -L "$RT")" 'D! 1'
check "remove reaction" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'reaction=0' $B/post/$RPID/react)" 303

# --- solved answers
check "other members cannot solve" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" $B/post/$RPID/solve)" 403
check "topic author marks answer" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/post/$RPID/solve)" 303
SP=$(curl -s -L "$RT")
has "answer link" "$SP" 'Jump to the answer'
has "qa json-ld" "$SP" 'QAPage'
has "answerer notified" "$(curl -s -b $W $B/notifications)" 'marked your post as the answer'
has "solved badge in list" "$(curl -s $B/latest)" '>Solved<'
check "unmark" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/post/$RPID/solve)" 303
no "answer cleared" "$(curl -s -L "$RT")" 'Jump to the answer'

# --- tag groups
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" -d 'tags=alpha,beta' $B/topic/$RTID/tags
check "create tag group" "$(adm -d '{"name":"Greek","one_per_topic":true,"tags":["alpha","beta"]}' $B/api/v1/admin/tag-groups)" 201
check "one per topic enforced" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'tags=alpha,beta' $B/topic/$RTID/tags)" 422
TG=$(curl -s -b $J $B/api/v1/admin/tag-groups | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "restrict to a forum" "$(adm -X PUT -d '{"name":"Greek","tags":["alpha"],"nodes":[1]}' $B/api/v1/admin/tag-groups/$TG)" 204
check "restricted tag refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'tags=alpha' $B/topic/$RTID/tags)" 422
check "delete tag group" "$(adm -X DELETE $B/api/v1/admin/tag-groups/$TG)" 204

# --- bookmark reminders
check "bad reminder refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'after=soon' $B/post/$RPID/remind)" 422
check "set reminder" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'after=1h' $B/post/$RPID/remind)" 303
has "reminder also bookmarks" "$(curl -s -L -b $W "$RT")" 'Bookmarked'

# --- stale answers, unsolved filter
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" $B/post/$RPID/solve
no "solved hidden from unsolved list" "$(curl -s "$B/forum/2-introductions/?show=unsolved&sort=newest")" 'Depth feature topic'
check "delete the answer" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/post/$RPID/delete)" 303
no "deleting the answer clears solved" "$(curl -s -L "$RT")" 'Jump to the answer'
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" $B/post/$RPID/restore

# --- promotion rules, warnings, announcements, directories
check "rule needs a condition" "$(adm -d '{"name":"R","set_trust_level":2}' $B/api/v1/admin/promotion-rules)" 422
check "rule cannot grant staff" "$(adm -d '{"name":"R","min_posts":1,"add_group":4}' $B/api/v1/admin/promotion-rules)" 422
check "create rule" "$(adm -d '{"name":"R","min_posts":1,"add_group":2}' $B/api/v1/admin/promotion-rules)" 201
WID=$(curl -s -b $J "$B/api/v1/admin/users?q=wordy" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "members cannot warn" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" -d 'points=1&reason=x' $B/profile/$WID/warn)" 403
check "reason required" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'points=1&reason=' $B/profile/$WID/warn)" 422
check "warn" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'points=5&reason=Be nice' $B/profile/$WID/warn)" 303
has "member sees warning" "$(curl -s -b $W $B/profile/$WID/warnings)" 'Be nice'
check "others cannot see warnings" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/profile/$WID/warnings)" 403
check "points silence automatically" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'body=Trying to post now' $B/topic/$RTID/reply)" 403
adm -X PATCH -d '{"silenced_until":null}' $B/api/v1/admin/users/$WID >/dev/null
check "announcement" "$(adm -d '{"title":"Big news","body":"**Read** <script>alert(8)</script>"}' $B/api/v1/admin/announcements)" 201
AP=$(curl -s $B/search)
has "announcement shown" "$AP" 'Big news'
no "announcement sanitized" "$AP" '<script>alert(8)'
check "future announcement" "$(adm -d '{"title":"Later news","body":"x","starts_at":"2999-01-01T00:00:00Z"}' $B/api/v1/admin/announcements)" 201
no "not yet shown" "$(curl -s $B/search)" 'Later news'
has "members directory" "$(curl -s "$B/members?sort=reputation")" 'wordy'
no "plugin accounts not listed" "$(curl -s $B/members)" 'plugin-ext'
has "staff directory" "$(curl -s $B/staff)" "$ADMINN"
check "online page" "$(curl -s -o /dev/null -w '%{http_code}' $B/online)" 200

# --- data export, deletion, merging
check "guests cannot export" "$(curl -s -o /dev/null -w '%{http_code}' $B/settings/export)" 403
EXPORT=$(curl -s -b $W $B/settings/export)
has "export has posts" "$EXPORT" 'Here is the full answer'
no "export has no password hash" "$EXPORT" 'password_hash'
check "deletion needs the username" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" -d 'confirm=wrong' $B/settings/delete)" 422
check "request deletion" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" -d 'confirm=pmoutsider' $B/settings/delete)" 303
check "cancel deletion" "$(curl -s -o /dev/null -w '%{http_code}' -b $K --data-urlencode "csrf=$T2" $B/settings/delete/cancel)" 303
OID=$(curl -s -b $J "$B/api/v1/admin/users?q=pmoutsider" | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "cannot merge an account into itself" "$(adm -d "{\"into\":$OID}" $B/api/v1/admin/users/$OID/merge)" 422
check "members cannot merge" "$(curl -s -o /dev/null -w '%{http_code}' -b $W -H "X-CSRF-Token: $TW" -H 'Content-Type: application/json' -d "{\"into\":$WID}" $B/api/v1/admin/users/$OID/merge)" 403
check "merge" "$(adm -d "{\"into\":$WID}" $B/api/v1/admin/users/$OID/merge)" 204
check "merged session ended" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/notifications)" 403
no "merged name gone" "$(curl -s -b $J "$B/api/v1/admin/users?q=pmoutsider")" 'pmoutsider'

# --- timers, slow mode, wiki, saved replies, inbound mail
check "members cannot set timers" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'action=close&hours=1' $B/topic/$RTID/timer)" 403
check "bad timer refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'action=explode&hours=1' $B/topic/$RTID/timer)" 422
check "set timer" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'action=close&hours=1' $B/topic/$RTID/timer)" 303
check "clear timer" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'action=none' $B/topic/$RTID/timer)" 303
adm -X PATCH -d '{"silenced_until":null,"suspended_until":null}' $B/api/v1/admin/users/$WID >/dev/null
check "slow mode on" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'seconds=3600' $B/topic/$RTID/slow)" 303
check "slow mode blocks a member's next post" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'body=Second reply too soon' $B/topic/$RTID/reply)" 422
check "moderators are not slowed" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'body=Staff reply goes through' $B/topic/$RTID/reply)" 303
curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" -d 'seconds=0' $B/topic/$RTID/slow
FP=$(grep -oE 'action="/post/[0-9]+/wiki"' <<<"$(curl -s -L -b $J "$RT")" | head -1 | grep -oE '[0-9]+')
check "members cannot edit before wiki" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'body=Vandalised first post' $B/post/$FP/edit)" 403
check "make wiki" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/post/$FP/wiki)" 303
setting wiki_min_trust_level 0; settle 1.5
check "trusted member edits wiki" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'body=Improved by the community' $B/post/$FP/edit)" 303
check "members cannot use saved replies" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/api/v1/saved-replies)" 403
check "add saved reply" "$(adm -d '{"title":"Welcome","body":"Welcome aboard"}' $B/api/v1/saved-replies)" 201
has "list saved replies" "$(curl -s -b $J $B/api/v1/saved-replies)" 'Welcome aboard'
check "save status" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'status=Away&hours=2' $B/settings/status)" 303
has "status on profile" "$(curl -s -L "$B/u/wordy")" 'Away'
check "signature image refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" --data-urlencode 'signature=![x](https://a.example/x.png)' $B/settings/signature)" 422
check "save signature" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" --data-urlencode 'signature=Sig **line**' $B/settings/signature)" 303
has "signature under posts" "$(curl -s -L "$RT")" 'class="cooked small muted signature"'
check "contact off by default" "$(curl -s -o /dev/null -w '%{http_code}' $B/contact)" 404
setting contact_email '"staff@example.com"'; settle 1.5
check "contact form opens" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/contact)" 200
check "contact sends" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'email=a@example.com&body=Hello+staff' $B/contact)" 200
check "bad action step refused" "$(adm -d '{"name":"Bad","steps":[{"kind":"explode"}]}' $B/api/v1/admin/mod-actions)" 422
check "save mod action" "$(adm -d '{"name":"Close it","steps":[{"kind":"lock"},{"kind":"reply","value":"Closed by staff"}]}' $B/api/v1/admin/mod-actions)" 201
MA=$(curl -s -b $J $B/api/v1/admin/mod-actions | grep -oE '"id":[0-9]+' | head -1 | grep -oE '[0-9]+')
check "members cannot run actions" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" $B/topic/$RTID/action/$MA)" 403
check "run mod action" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/topic/$RTID/action/$MA)" 303
has "action replied" "$(curl -s -L "$RT")" 'Closed by staff'
check "bulk mail needs a subject" "$(adm -d '{"subject":"","body":"x"}' $B/api/v1/admin/bulk-mail)" 422
check "bulk mail queued" "$(adm -d '{"subject":"News","body":"Hello all"}' $B/api/v1/admin/bulk-mail)" 202
check "clubs off by default" "$(curl -s -o /dev/null -w '%{http_code}' $B/clubs)" 404
setting clubs_enabled true; settle 1.5
setting club_min_trust_level 1; setting clubs_per_member 3; settle 1.5
check "create open club" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'name=Chess+Club&open=1' $B/clubs)" 303
check "create closed club" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'name=Secret+Club' $B/clubs)" 303
CL=$(curl -s -b $W $B/clubs)
has "club listed" "$CL" 'Chess Club'
CJ=$(grep -oE 'action="/clubs/[0-9]+/join"' <<<"$CL" | head -1 | grep -oE '[0-9]+')
check "member joins" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" $B/clubs/$CJ/join)" 303
has "manifest" "$(curl -s $B/manifest.webmanifest)" '"display":"standalone"'
check "service worker" "$(curl -s -o /dev/null -w '%{http_code}' $B/sw.js)" 200
check "unknown address is still 404" "$(curl -s -o /dev/null -w '%{http_code}' $B/index.php/nope)" 404
# --- paths that only run on a tick or by mail (need e2e-postgres.sh's settings)
if [ -n "${BF_INBOUND_TOKEN:-}" ]; then
  IN() { curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $2" -H 'Content-Type: application/json' -d "$1" $B/api/v1/webhooks/inbound-mail; }
  check "inbound needs the token" "$(IN '{"to":"reply+x@a.b","text":"hi"}' wrong)" 403
  check "unknown key is dropped quietly" "$(IN '{"to":"reply+nope@a.b","text":"hi"}' $BF_INBOUND_TOKEN)" 200
  curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" -d 'action=none' $B/topic/$RTID/timer
  adm -X PATCH -d '{"silenced_until":null,"suspended_until":null}' $B/api/v1/admin/users/$WID >/dev/null
  curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" -d 'action=unlock' $B/topic/$RTID/mod
  curl -s -o /dev/null -b $W --data-urlencode "csrf=$TW" -d 'level=watch' $B/topic/$RTID/follow
  curl -s -o /dev/null -b $W --data-urlencode "csrf=$TW" -d 'reply_app=on&reply_email=on' $B/settings/notifications
  setting email_delay_minutes 0; settle 1.5
  curl -s -o /dev/null -b $J --data-urlencode "csrf=$T" -d 'body=Please answer me by email' $B/topic/$RTID/reply
  for _ in $(seq 40); do RF=$(grep -l 'To: wordy@' "$MAIL"/*.eml 2>/dev/null | xargs grep -l 'Depth feature topic' 2>/dev/null | xargs grep -l 'Reply-To: reply+' 2>/dev/null | sort | tail -1); [ -n "$RF" ] && break; sleep 0.5; done
  RK=$(grep -h 'Reply-To: reply+' "$RF" | sed -E 's/.*reply\+([^@]+)@.*/\1/')
  has "topic mail has a reply address" "$RK" '-'
  check "reply by email accepted" "$(IN "{\"to\":\"reply+$RK@mail.example\",\"text\":\"Answered from my inbox\\n\\nOn Mon, x wrote:\\n> quoted original\"}" $BF_INBOUND_TOKEN)" 200
  EP=$(curl -s -L "$RT")
  has "emailed reply posted" "$EP" 'Answered from my inbox'
  no "quoted text left out" "$EP" 'quoted original'
  no "Reply-To has no mail address" "$RK" '@'
  adm -X PATCH -d '{"suspended_until":"2099-01-01T00:00:00Z"}' $B/api/v1/admin/users/$WID >/dev/null
  IN "{\"to\":\"reply+$RK@mail.example\",\"text\":\"Posted while suspended\"}" $BF_INBOUND_TOKEN >/dev/null
  no "suspended member cannot reply by email" "$(curl -s -L "$RT")" 'Posted while suspended'
  adm -X PATCH -d '{"suspended_until":null}' $B/api/v1/admin/users/$WID >/dev/null
  # Suspending ended the session, as it should.
  curl -s -o /dev/null -c $W -b $W $B/login; TW=$(csrf $W)
  curl -s -o /dev/null -c $W -b $W --data-urlencode "csrf=$TW" --data-urlencode 'login=wordy' --data-urlencode 'password=another long passphrase' $B/login

  check "anonymous off by default" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'anonymous=1&body=Named because it is off' $B/topic/$RTID/reply)" 303
  no "off means named" "$(curl -s -L "$RT" | grep -B6 'Named because it is off' | grep -c '>Anonymous<' | grep -v '^0$')" '1'
  setting anonymous_posting true; settle 1.5
  check "anonymous reply" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'anonymous=1&body=A quiet secret reply' $B/topic/$RTID/reply)" 303
  has "shown as anonymous" "$(curl -s -L "$RT")" '<strong>Anonymous</strong>'
  NF=$(curl -s -b $J $B/notifications)
  has "watchers told it was anonymous" "$NF" 'Anonymous posted in'
  setting approve_first_posts 50; settle 1.5
  curl -s -o /dev/null -b $W --data-urlencode "csrf=$TW" -d 'anonymous=1&body=Held anonymous reply here' $B/topic/$RTID/reply
  setting approve_first_posts 0; settle 1.5
  has "held anonymous reply stays anonymous in review" "$(curl -s -b $J $B/review | grep -A12 'Held anonymous reply' | head -40; curl -s -b $J $B/review)" 'Anonymous'
  setting anonymous_posting false

  # A moderator's scheduled topic stays private until its timer fires.
  check "schedule a topic" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" --data-urlencode 'title=Scheduled news topic' --data-urlencode 'body=This goes public later' -d 'publish_minutes=1' $B/forum/2/new)" 303
  has "scheduled queue" "$(curl -s -b $J $B/scheduled)" 'Scheduled news topic'
  no "not public yet" "$(curl -s $B/latest)" 'Scheduled news topic'

  TM=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode 'title=Timed close topic' --data-urlencode 'body=This one closes itself' $B/forum/2/new)
  TMID=$(grep -oE '/topic/[0-9]+' <<<"$TM" | grep -oE '[0-9]+')
  check "close in a minute" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'action=close&minutes=1' $B/topic/$TMID/timer)" 303
  TD=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode 'title=Timed delete topic' --data-urlencode 'body=This one deletes itself' $B/forum/2/new)
  TDID=$(grep -oE '/topic/[0-9]+' <<<"$TD" | grep -oE '[0-9]+')
  check "delete in a minute" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'action=delete&minutes=1' $B/topic/$TDID/timer)" 303
  check "remind in a minute" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'after=1min' $B/post/$RPID/remind)" 303
  reg gonesoon >/dev/null; D=$(mktemp); curl -s -c $D -b $D -o /dev/null $B/login; TD2=$(csrf $D)
  curl -s -o /dev/null -c $D -b $D --data-urlencode "csrf=$TD2" --data-urlencode 'login=gonesoon' --data-urlencode 'password=another long passphrase' $B/login
  check "delete account now" "$(curl -s -o /dev/null -w '%{http_code}' -b $D --data-urlencode "csrf=$TD2" -d 'confirm=gonesoon' $B/settings/delete)" 303
  # Levels 1 and 2 need one post and nothing else, so the promotion job raises the member to level 2.
  for k in days days_visited topics_read reputation likes_received likes_given; do
    setting trust_level_1_$k 0; setting trust_level_2_$k 0
  done
  setting trust_level_1_posts 1; setting trust_level_2_posts 1
  # Timers were set one minute ahead; with database access, make them due now rather than wait.
  if [ -n "${BF_TEST_DB_HOST:-}" ]; then
    psql -q -h "$BF_TEST_DB_HOST" -U forum -d forum_e2e -c "UPDATE item_timers SET run_at = now(); UPDATE bookmarks SET remind_at = now() WHERE remind_at IS NOT NULL"
    settle 40
  else
    settle 90
  fi
  has "timer closed the topic" "$(curl -s -L "$TM")" '>Locked<'
  check "timer deleted the topic" "$(curl -s -o /dev/null -w '%{http_code}' -L "$TD")" 404
  has "reminder delivered" "$(curl -s -b $W $B/notifications)" 'Reminder about a post you bookmarked'
  has "scheduled topic published" "$(curl -s $B/latest)" 'Scheduled news topic'
  no "account anonymized" "$(curl -s -b $J "$B/api/v1/admin/users?q=gonesoon")" 'gonesoon'
  check "deleted account signed out" "$(curl -s -o /dev/null -w '%{http_code}' -b $D $B/notifications)" 403
  setting wiki_min_trust_level 2; settle 1.5
  check "promotion raised the trust level" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'body=Edited after promotion' $B/post/$FP/edit)" 303
fi

# --- remaining community tools
check "folder" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'folder=Work' $B/messages/$PM/folder)" 303
has "folder listed" "$(curl -s -b $J $B/messages)" 'folder=Work'
has "folder filters" "$(curl -s -b $J "$B/messages?folder=Work")" 'Secret plans'
check "outsider cannot file it" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'folder=x' $B/messages/$PM/folder)" 404
SU=$(curl -s -o /dev/null -w '%{redirect_url}' -b $W --data-urlencode "csrf=$TW" --data-urlencode 'to=@staff' --data-urlencode 'subject=Help staff' --data-urlencode 'body=A question for staff' $B/messages)
has "message to staff" "$SU" '/messages/'
SM=$(grep -oE '[0-9]+$' <<<"$SU")
has "staff received it" "$(curl -s -b $J $B/messages)" 'Help staff'
check "staff cannot read others' messages" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/messages/$SM/nope)" 404
check "report a conversation" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'reason=This+is+rude' $B/messages/$SM/report)" 303
check "members have no reports queue" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/review/messages)" 403
RQ=$(curl -s -b $J $B/review/messages)
has "report queued" "$RQ" 'This is rude'
RID=$(grep -oE '/review/messages/[0-9]+' <<<"$RQ" | head -1 | grep -oE '[0-9]+$')
has "staff read through report" "$(curl -s -b $J $B/review/messages/$RID)" 'A question for staff'
has "reading is logged" "$(curl -s -b $J "$B/api/v1/admin/audit?action=conversation.")" 'conversation.read_reported'
check "close report" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/review/messages/$RID)" 303
check "closed report no longer opens" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/review/messages/$RID)" 404
check "cannot follow yourself" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" $B/profile/$WID/follow)" 422
check "follow a member" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/profile/$WID/follow)" 303
has "following feed" "$(curl -s -b $J $B/following)" 'Depth feature topic'
has "member summary" "$(curl -sL $B/profile/$WID/summary)" 'Top posts'
has "topic summary page" "$(curl -s $B/topic/$RTID/summary)" 'Back to the full topic'
check "banner" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" $B/topic/$RTID/banner)" 303
has "banner shown" "$(curl -s $B/search)" 'Read the topic'
check "do not disturb" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'hours=1' $B/settings/dnd)" 303
has "accounts by ip" "$(curl -s -b $J $B/api/v1/admin/ip/127.0.0.1)" '"username"'
check "bad ip refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/api/v1/admin/ip/nope)" 422
has "member history" "$(curl -s -b $J $B/api/v1/admin/users/$WID/history)" 'user.warn'
has "report data" "$(curl -s -b $J $B/api/v1/admin/reports/posts)" '"count"'
check "unknown report" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B/api/v1/admin/reports/nope)" 404
check "bulk cannot grant staff" "$(adm -d '{"group":4}' $B/api/v1/admin/bulk-group)" 422
check "group inbox closed to outsiders" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" --data-urlencode 'to=@helpers' -d 'subject=Hi&body=Hello+everyone' $B/messages)" 422
has "rank on profile" "$(curl -s -L $B/u/wordy)" '<dt>Rank</dt>'
check "stream needs a name" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'name=' $B/streams)" 422
check "save stream" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'name=Mine&unsolved=1' $B/streams)" 303
STR=$(grep -oE '/streams/[0-9]+' <<<"$(curl -s -b $W $B/streams)" | head -1)
check "stream opens" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B$STR)" 200
check "streams are private" "$(curl -s -o /dev/null -w '%{http_code}' -b $J $B$STR)" 404
has "onboarding" "$(curl -s -b $W $B/welcome)" 'Write your first post'
check "opt in to visitors" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'on=1' $B/settings/visitors)" 303
curl -s -o /dev/null -L -b $J $B/u/wordy
has "visitor listed" "$(curl -s -b $W $B/settings/visitors)" "$ADMINN"
check "opt out" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" $B/settings/visitors)" 303
no "opting out clears the list" "$(curl -s -b $W $B/settings/visitors)" "$ADMINN"
# --- hidden blocks
HT=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode 'title=Hidden block topic' --data-urlencode 'body=Visible zanzibar part [hide]quokkasecret words[/hide] end' $B/forum/2/new)
HTID=$(grep -oE '/topic/[0-9]+' <<<"$HT" | grep -oE '[0-9]+')
no "guests do not see hidden text" "$(curl -s -L "$HT")" 'quokkasecret'
has "guests see the marker" "$(curl -s -L "$HT")" 'hidden content'
has "author sees hidden text" "$(curl -s -L -b $J "$HT")" 'quokkasecret'
no "members who have not replied do not" "$(curl -s -L -b $W "$HT")" 'quokkasecret'
no "search index skips it" "$(curl -s "$B/api/v1/search?q=quokkasecret")" 'Hidden block topic'
has "the rest is searchable" "$(curl -s "$B/api/v1/search?q=zanzibar")" 'Hidden block topic'
no "rss has no hidden text" "$(curl -s $B/latest.rss)" 'quokkasecret'
HP=$(grep -oE 'data-quote="[0-9]+"' <<<"$(curl -s -L -b $W "$HT")" | head -1 | grep -oE '[0-9]+')
no "quote source has no hidden text" "$(curl -s -b $W $B/api/v1/posts/$HP/raw)" 'quokkasecret'
curl -s -o /dev/null -b $W --data-urlencode "csrf=$TW" -d 'body=Replying to see the hidden part' $B/topic/$HTID/reply
has "replying reveals it" "$(curl -s -L -b $W "$HT")" 'quokkasecret'
check "group limit for messages" "$(adm -X PUT -d '{"pm_max_participants":2}' $B/api/v1/admin/groups/2/limits)" 204
adm -X PUT -d '{}' $B/api/v1/admin/groups/2/limits >/dev/null
has "arabic is right to left" "$(curl -s -H 'Accept-Language: ar' $B/search)" 'dir="rtl"'
check "embedding off by default" "$(curl -s -o /dev/null -w '%{http_code}' "$B/embed/comments?url=https://blog.example/a")" 404
setting embed_hosts '"127.0.0.1"'; setting embed_forum 2; settle 1.5
check "unlisted host refused" "$(curl -s -o /dev/null -w '%{http_code}' "$B/embed/comments?url=https://evil.example/a")" 404
if [ -n "${OIDC:-}" ]; then
  check "a page that does not exist gets no thread" "$(curl -s -o /dev/null -w '%{http_code}' "$B/embed/comments?url=http://127.0.0.1:1/nope")" 404
  EH=$(curl -s -D - "$B/embed/comments?url=$OIDC/hook-log")
  has "embed frames only for listed hosts" "$EH" 'frame-ancestors https://127.0.0.1'
  has "embed links to the topic" "$EH" 'Join the discussion'
fi
if [ -n "${BF_VAPID_PRIVATE:-}" ]; then
  has "push key offered" "$(curl -s $B/api/v1/push/key)" '"key"'
  SUB() { curl -s -o /dev/null -w '%{http_code}' -b $W -H "X-CSRF-Token: $TW" -H 'Content-Type: application/json' -d "{\"endpoint\":\"$1\",\"keys\":{\"p256dh\":\"BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4\",\"auth\":\"BTBZMqHH6r4Tts7J_aSIgg\"}}" $B/api/v1/push/subscribe; }
  check "push address must be https" "$(SUB file:///etc/passwd)" 422
  for i in 1 2 3 4 5 6 7 8 9 10; do SUB "https://push.invalid/$i" >/dev/null; done
  check "push subscriptions are capped" "$(SUB https://push.invalid/11)" 422
fi
# Founding a club gives power over that club only.
setting club_min_trust_level 0; settle 1.5
check "member founds a club" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'name=Wordy+Club&open=1' $B/clubs)" 303
sleep 1
check "founder cannot read reported messages" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/review/messages)" 403
check "founder cannot warn people" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'points=1&reason=x' $B/profile/1/warn)" 403
check "founder cannot use saved replies" "$(curl -s -o /dev/null -w '%{http_code}' -b $W $B/api/v1/saved-replies)" 403
setting clubs_per_member 1; settle 1.5
check "club limit" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'name=Second+Club' $B/clubs)" 422
# --- first-party features
check "badge needs a condition" "$(adm -d '{"name":"X"}' $B/api/v1/admin/badges)" 422
check "create badge" "$(adm -d '{"name":"First steps","description":"Posted once","min_posts":1,"points":5}' $B/api/v1/admin/badges)" 201
PT=$(curl -s -b $J -o /dev/null -w '%{redirect_url}' --data-urlencode "csrf=$T" --data-urlencode 'title=Poll and date topic' --data-urlencode 'body=Please vote and come along' $B/forum/2/new)
PTID=$(grep -oE '/topic/[0-9]+' <<<"$PT" | grep -oE '[0-9]+')
check "poll needs two options" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'question=Q&options=one' $B/topic/$PTID/poll)" 422
check "others cannot add polls" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" --data-urlencode 'question=Q' --data-urlencode $'options=a\nb' $B/topic/$PTID/poll)" 403
check "create poll" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" --data-urlencode 'question=Tea or coffee?' --data-urlencode $'options=Tea\nCoffee <b>' $B/topic/$PTID/poll)" 303
PP=$(curl -s -L -b $W "$PT")
has "poll shown" "$PP" 'Tea or coffee?'
has "poll option escaped" "$PP" 'Coffee &lt;b&gt;'
OPT=$(grep -oE 'name="option" value="[0-9]+"' <<<"$PP" | head -1 | grep -oE '[0-9]+')
check "vote" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d "option=$OPT" $B/topic/$PTID/vote)" 303
has "vote counted" "$(curl -s -L -b $W "$PT")" 'your vote'
check "foreign option refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $W --data-urlencode "csrf=$TW" -d 'option=999999' $B/topic/$PTID/vote)" 404
check "assign to a non-moderator refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'username=wordy' $B/topic/$PTID/assign)" 422
check "assign to self" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" --data-urlencode "username=$ADMINN" $B/topic/$PTID/assign)" 303
has "assigned list" "$(curl -s -b $J $B/assigned)" 'Poll and date topic'
check "members have no assigned list" "$(curl -s -o /dev/null -w '%{http_code}' -b $K $B/assigned)" 403
check "bad date refused" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'date=soon' $B/topic/$PTID/event)" 422
check "add event" "$(curl -s -o /dev/null -w '%{http_code}' -b $J --data-urlencode "csrf=$T" -d 'date=2999-01-01' $B/topic/$PTID/event)" 303
has "calendar" "$(curl -s $B/calendar)" 'Poll and date topic'
has "direction attribute" "$(curl -s $B/search)" 'dir="ltr"'

rm -f $J $K
exit $fail
