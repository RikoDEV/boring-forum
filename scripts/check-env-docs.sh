#!/bin/sh
# Fails when the server reads an environment variable that the docs or .env.example
# does not mention. Names are the quoted BF_* and OTEL_* strings in crates/**/*.rs.
#   scripts/check-env-docs.sh
set -eu
cd "$(dirname "$0")/.."

# Used by tests and tooling only, not by an operator.
ignore=' BF_DEVDB '

names=$(grep -rhoE '"(BF_|OTEL_)[A-Z0-9_]+"' crates --include='*.rs' | tr -d '"' | sort -u)
missing=0
for n in $names; do
  case "$ignore" in *" $n "*) continue ;; esac
  if ! grep -q "\`$n\`" docs/*.md; then
    echo "docs/ does not document $n"
    missing=1
  fi
  if ! grep -q "^#\{0,1\} \{0,1\}$n=" .env.example; then
    echo ".env.example does not list $n"
    missing=1
  fi
done
if [ "$missing" -ne 0 ]; then
  echo "Add each variable to the table in docs/configuration.md (in backticks) and to .env.example." >&2
  exit 1
fi
echo "environment variables: all documented"
