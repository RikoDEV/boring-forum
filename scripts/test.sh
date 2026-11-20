#!/usr/bin/env bash
# Runs the tests on Linux in Docker, with Postgres in memory and shortened server timers.
#   scripts/test.sh                 the end-to-end suite (Vitest)
#   scripts/test.sh core            only spec files matching "core"
#   scripts/test.sh --unit          cargo test
#   scripts/test.sh --all           cargo test, then end-to-end
# Set BF_TEST_DOCKER to change how Docker is called, for example through WSL:
#   BF_TEST_DOCKER="env MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu --cd /mnt/c/path/to/repo -- docker"
cd "$(dirname "$0")/.."
D=${BF_TEST_DOCKER:-docker}
# `npm ci` installs exactly the committed lockfile and never rewrites it, so a test run cannot
# silently pull a newer (possibly malicious) version within a range.
e2e='cd tests/e2e && npm ci --no-audit --no-fund --silent && npx vitest run'
case "${1:-}" in
  --unit) inner='cargo test --workspace' ;;
  --all) inner="cargo test --workspace && $e2e" ;;
  *)
    # Each argument is a spec-name fragment. Refuse one that matches no spec file, so a typo
    # fails here instead of running nothing.
    for f in "$@"; do
      if ! find tests/e2e/specs -name '*.test.ts' | grep -qF -- "$f"; then
        echo "scripts/test.sh: no spec file matches '$f' (spec files are in tests/e2e/specs)" >&2
        exit 2
      fi
    done
    inner="$e2e $*" ;;
esac
$D compose -f deploy/compose.test.yml run --rm --build runner bash -c "$inner"
status=$?
$D compose -f deploy/compose.test.yml down >/dev/null 2>&1
exit $status
