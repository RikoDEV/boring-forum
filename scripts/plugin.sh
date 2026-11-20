#!/bin/sh
# Plugin development helper (docs/plugins/getting-started.md):
#   scripts/plugin.sh pack  <dir> [out.zip]         build a plugin folder into an installable zip (default <dir>/dist/plugin.zip)
#   scripts/plugin.sh sign  <zip> <seed-hex>        add signature.txt (64 hex characters = an Ed25519 seed; prints the public key)
#   scripts/plugin.sh watch <dir> <site-url> <key>  rebuild and reinstall on every change, with an admin API key
# A folder with a Cargo.toml is built for wasm32-unknown-unknown first (rustup target add wasm32-unknown-unknown).
set -eu
cd "$(dirname "$0")/.."
# Paths are given relative to where you called the script.
case "${1:-}" in
  pack|sign|watch) cmd=$1; shift ;;
  *) sed -n '2,6p' "$0"; exit 2 ;;
esac
first=$(cd "$OLDPWD" 2>/dev/null && realpath "$1" 2>/dev/null || echo "$1")
shift
exec cargo run -q -p bf-server --bin boring-forum -- plugin "$cmd" "$first" "$@"
