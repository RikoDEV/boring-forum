#!/bin/sh
# Sets the workspace version for a release (run by hand before tagging; see docs/releasing.md).
#   scripts/set-version.sh 1.2.3
set -eu
cd "$(dirname "$0")/.."
version="${1:?usage: set-version.sh <version>}"
# The first `version = ` in the root manifest is [workspace.package]; every crate inherits it.
sed -i "0,/^version = \".*\"/s//version = \"$version\"/" Cargo.toml
# Moves the workspace crates in Cargo.lock to the new version without touching dependencies.
cargo update --workspace
