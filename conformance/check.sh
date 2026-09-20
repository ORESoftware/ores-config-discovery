#!/bin/sh
set -eu
root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
cd "$root"
fail(){ echo "[ores-config-discovery conformance] $*" >&2; exit 1; }
for boundary in contracts conformance; do
  [ -d "$boundary" ] && [ ! -L "$boundary" ] || fail "$boundary/ must be a real non-symlink directory"
done
escaped=$(find contracts conformance -type l -print -quit 2>/dev/null || true)
[ -z "$escaped" ] || fail "symlink inside contract/conformance boundary: $escaped"
[ -f conformance/consumer-pins.toml ] || fail "missing conformance/consumer-pins.toml"
command -v cargo >/dev/null 2>&1 || fail "cargo is required"
cargo test --locked --all-targets
