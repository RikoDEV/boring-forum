#!/bin/sh
# Builds the plugin and packs dist/community-kit.zip, ready for Admin > Plugins > Install.
# Needs `rustup target add wasm32-unknown-unknown`. From the repo root: scripts/plugin.sh pack examples/plugins/community-kit
set -eu
cd "$(dirname "$0")"
exec ../../../../scripts/plugin.sh pack .
