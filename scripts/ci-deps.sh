#!/usr/bin/env bash
# System packages + Rust toolchain for CI (Ubuntu/Debian runners, GitHub or Forgejo).
set -euo pipefail
SUDO=""; command -v sudo >/dev/null && [ "$(id -u)" != 0 ] && SUDO=sudo
if [ "${1:-}" = "gui" ]; then
  $SUDO apt-get update -qq
  $SUDO apt-get install -y -qq --no-install-recommends \
    build-essential pkg-config curl file libssl-dev libudev-dev \
    libwebkit2gtk-4.1-dev libsoup-3.0-dev libgtk-3-dev librsvg2-dev libxdo-dev >/dev/null
else
  command -v cc >/dev/null || { $SUDO apt-get update -qq && $SUDO apt-get install -y -qq --no-install-recommends build-essential pkg-config curl >/dev/null; }
fi
if ! command -v cargo >/dev/null; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy,rustfmt
  echo "$HOME/.cargo/bin" >> "${GITHUB_PATH:-/dev/null}"
  export PATH="$HOME/.cargo/bin:$PATH"
fi
rustup component add clippy rustfmt >/dev/null 2>&1 || true
cargo --version
