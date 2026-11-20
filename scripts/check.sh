#!/bin/sh
# One command for the checks a change must pass before review (see CONTRIBUTING.md).
#   scripts/check.sh          everything below
#   scripts/check.sh --fast   skip the test run
# The Postgres tests are not included: cargo test -p bf-db -- --ignored (needs BF_DATABASE_URL).
set -eu
cd "$(dirname "$0")/.."

fast=0
[ "${1:-}" = "--fast" ] && fast=1

step() { printf '\n==> %s\n' "$*"; }

step "cargo fmt --check"
cargo fmt --check

step "cargo clippy -D warnings"
cargo clippy --locked --workspace --all-targets -- -D warnings

if [ "$fast" -eq 0 ]; then
  if command -v cargo-nextest >/dev/null 2>&1; then
    step "cargo nextest"
    cargo nextest run --locked --workspace
  else
    step "cargo test (cargo install cargo-nextest --locked gives the CI runner)"
    cargo test --locked --workspace
  fi
fi

step "environment variables"
sh scripts/check-env-docs.sh

step "translations"
# `python3` on Windows can be a Store stub that exits without running anything: try it first.
if python3 -c "" >/dev/null 2>&1; then py=python3; else py=python; fi
"$py" scripts/i18n.py check
"$py" scripts/test_i18n.py

if command -v cargo-machete >/dev/null 2>&1; then
  step "cargo machete"
  cargo machete
else
  echo "(skipping cargo machete: cargo install cargo-machete --locked)"
fi

if [ -f web/package.json ] && command -v pnpm >/dev/null 2>&1 && [ -d web/node_modules ]; then
  step "web checks"
  (cd web && pnpm check && node scripts/size-check.mjs)
else
  echo "(skipping web checks: run 'pnpm install' in web/ and have pnpm on PATH)"
fi

printf '\nAll checks passed.\n'
