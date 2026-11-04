#!/usr/bin/env bash
# Everything CI checks, runnable locally: ./scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."
echo "== cpmm (CLI)"
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
echo "== gui (frontend)"
( cd gui && npm ci --no-audit --no-fund >/dev/null && npx biome check . && npx tsc -b && npx vite build >/dev/null )
echo "== gui (Tauri backend)"
( cd gui/src-tauri && cargo fmt --check && cargo clippy --locked -- -D warnings )
echo "all checks passed"
