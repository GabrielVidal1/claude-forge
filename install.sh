#!/usr/bin/env sh
# Install claude-forge by downloading the matching release binary from GitHub
# into ~/.local/bin (override with CLAUDE_FORGE_BIN_DIR).
#
#   curl -fsSL https://raw.githubusercontent.com/GabrielVidal1/claude-forge/main/install.sh | sh
set -eu

REPO="GabrielVidal1/claude-forge"
BIN="claude-forge"
BIN_DIR="${CLAUDE_FORGE_BIN_DIR:-$HOME/.local/bin}"

# --- detect OS/arch and map to a release target triple ---
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)  os_part="unknown-linux-musl" ;;
  Darwin) os_part="apple-darwin" ;;
  *) echo "unsupported OS: $os" >&2; exit 1 ;;
esac

case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) echo "unsupported arch: $arch" >&2; exit 1 ;;
esac

target="${arch_part}-${os_part}"
asset="${BIN}-${target}"

# --- resolve the latest release tag ---
tag="${CLAUDE_FORGE_VERSION:-}"
if [ -z "$tag" ]; then
  tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
fi
if [ -z "$tag" ]; then
  echo "could not determine latest release tag" >&2
  exit 1
fi

url="https://github.com/${REPO}/releases/download/${tag}/${asset}"
echo "Downloading ${BIN} ${tag} (${target})..."

mkdir -p "$BIN_DIR"
tmp="$(mktemp)"
if ! curl -fSL "$url" -o "$tmp"; then
  echo "download failed: $url" >&2
  echo "(no prebuilt binary for ${target}? install with: cargo install --git https://github.com/${REPO})" >&2
  rm -f "$tmp"
  exit 1
fi
chmod +x "$tmp"
mv "$tmp" "$BIN_DIR/$BIN"

echo "Installed ${BIN} to ${BIN_DIR}/${BIN}"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "note: ${BIN_DIR} is not on your PATH — add it to use '${BIN}' directly." ;;
esac
