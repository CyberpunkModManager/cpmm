#!/usr/bin/env bash
# Build cpmm and cpmm-gui from this checkout and install them, for running the working tree
# instead of a release: ./scripts/install-local.sh
#
# Installs into ~/.local/bin (override with CPMM_INSTALL_DIR). Each binary is copied in as
# <name>.new and renamed over the old one, so a running GUI or game session keeps the file
# it started with and picks up the new build on its next start.
set -euo pipefail
cd "$(dirname "$0")/.."

dest="${CPMM_INSTALL_DIR:-$HOME/.local/bin}"
for d in "$HOME/.cargo/bin" "$HOME/.local/share/node/bin"; do
  [ -d "$d" ] && PATH="$d:$PATH"
done
command -v cargo >/dev/null || { echo "cargo not found (install Rust: https://rustup.rs)" >&2; exit 1; }
command -v npm >/dev/null || { echo "npm not found (install Node 22+)" >&2; exit 1; }

echo "== cpmm"
cargo build --release --locked

echo "== cpmm-gui"
(
  cd gui
  # npm ci only when the dependencies are missing or the lockfile changed since
  if [ ! -d node_modules ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then
    npm ci --no-audit --no-fund
  fi
  npx tauri build --no-bundle
)

mkdir -p "$dest"
install_bin() {
  install -m 755 "$1" "$dest/$2.new"
  mv -f "$dest/$2.new" "$dest/$2"
}
install_bin target/release/cpmm cpmm
install_bin gui/src-tauri/target/release/cpmm-gui cpmm-gui

rev="$(git describe --always --dirty 2>/dev/null || echo unknown)"
echo "installed cpmm and cpmm-gui ($rev) into $dest"
if pgrep -f "$dest/cpmm-gui" >/dev/null; then
  echo "cpmm-gui is running: restart it to use the new build"
fi
