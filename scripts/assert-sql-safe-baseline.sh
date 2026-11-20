#!/usr/bin/env bash
# Checks (or regenerates) the baseline of unexplained `sqlx::AssertSqlSafe` uses
#.
#
#   bash scripts/assert-sql-safe-baseline.sh           check; non-zero when a new one appears
#   bash scripts/assert-sql-safe-baseline.sh --write    record the current state as the baseline
#
# `AssertSqlSafe` turns off sqlx's compile-time injection check, so every use needs a comment on
# the line above saying why the string cannot contain attacker-controlled data. Thirty-eight
# uses predate that rule. Rather than fail every pull request until all of them are annotated --
# which would end with the check being deleted -- this compares per-file counts against a
# committed baseline: a new unexplained use fails, the existing ones are recorded as debt.
set -euo pipefail
cd "$(dirname "$0")/.."

BASELINE=.github/assert-sql-safe-baseline.txt

# One line per unexplained use, as "<count> <file>".
current() {
    grep -rnE 'AssertSqlSafe' crates --include='*.rs' | grep -v '/tests/' | while IFS=: read -r file line _; do
        prev=$((line - 1))
        # A `//` comment directly above counts as the explanation.
        sed -n "${prev}p" "$file" | grep -qE '^\s*//' || echo "$file"
    done | sort | uniq -c | awk '{printf "%s %s\n", $1, $2}'
}

if [ "${1:-}" = "--write" ]; then
    {
        sed -n '1,/^# <count> <file>$/p' "$BASELINE" 2>/dev/null || true
        current
    } > "$BASELINE.tmp"
    mv "$BASELINE.tmp" "$BASELINE"
    echo "baseline written to $BASELINE"
    exit 0
fi

want=$(grep -vE '^\s*(#|$)' "$BASELINE" | sort)
have=$(current | sort)

if [ "$want" = "$have" ]; then
    total=$(echo "$want" | awk '{ n += $1 } END { print n + 0 }')
    echo "OK: $total unexplained AssertSqlSafe use(s), unchanged from the baseline"
    exit 0
fi

echo "AssertSqlSafe baseline mismatch." >&2
echo >&2
diff <(echo "$want") <(echo "$have") | sed 's/^</  baseline:/; s/^>/  now:     /' >&2
echo >&2
cat >&2 <<'MSG'
A count that went UP means a new `AssertSqlSafe` without a comment above it saying why the
string cannot contain attacker-controlled data. Add the comment, or use bind parameters.

A count that went DOWN is good news: run `bash scripts/assert-sql-safe-baseline.sh --write`
and commit the smaller baseline.
MSG
exit 1
