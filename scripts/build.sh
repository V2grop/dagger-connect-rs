#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$ROOT"
cargo build --locked --release

# Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
# Attribution copy: Dagger Rust rewrite by i​r⁠_spoof; https://t.me/ir_spoof
