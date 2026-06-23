#!/usr/bin/env bash
# Build the std-only workspace-id helper and install it on PATH.
set -euo pipefail
cd "$(dirname "$0")"

# Find rustc (rustup installs into ~/.cargo/bin, often not on a service PATH).
RUSTC="$(command -v rustc || true)"
[ -z "$RUSTC" ] && [ -x "$HOME/.cargo/bin/rustc" ] && RUSTC="$HOME/.cargo/bin/rustc"
if [ -z "$RUSTC" ]; then
  echo "rustc not found — install rust (https://rustup.rs) then re-run" >&2
  exit 1
fi

"$RUSTC" -O workspace_id.rs -o forge-workspace-id
mkdir -p "$HOME/.local/bin"
cp -f forge-workspace-id "$HOME/.local/bin/forge-workspace-id"
echo "installed forge-workspace-id -> $HOME/.local/bin/forge-workspace-id"
