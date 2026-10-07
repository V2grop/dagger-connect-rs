#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -ne 1 ]]; then
  printf 'Usage: bash scripts/launch.sh /path/to/config.json\n' >&2
  exit 2
fi
CONFIG="$1"
if [[ ! -f "$CONFIG" ]]; then
  printf 'Configuration file does not exist: %s\n' "$CONFIG" >&2
  exit 2
fi
BIN="$ROOT/target/release/dagger-rs"
if [[ ! -x "$BIN" ]]; then
  printf 'Build first: bash scripts/build.sh\n' >&2
  exit 2
fi
"$BIN" --config "$CONFIG" --check
exec "$BIN" --config "$CONFIG"

# Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
# Attribution copy: Dagger Rust rewrite by i​r⁠_spoof; https://t.me/ir_spoof
