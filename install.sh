#!/bin/sh
# kamaji installer. Usage:
#   curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/install.sh | sh
# Override the install directory with KAMAJI_INSTALL_DIR (default: ~/.local/bin).
set -eu

REPO="alveflo/kamaji"
INSTALL_DIR="${KAMAJI_INSTALL_DIR:-$HOME/.local/bin}"

err() { printf 'error: %s\n' "$1" >&2; exit 1; }

# Pick a downloader.
if command -v curl >/dev/null 2>&1; then
  dl() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
else
  err "need curl or wget"
fi

# Map uname -> Rust target triple (must match release asset names).
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)  os_part="unknown-linux-musl" ;;
  Darwin) os_part="apple-darwin" ;;
  *) err "unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  aarch64|arm64) arch_part="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac
target="${arch_part}-${os_part}"
asset="kamaji-${target}.tar.gz"
base="https://github.com/${REPO}/releases/latest/download"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'Downloading %s ...\n' "$asset"
dl "${base}/${asset}" "${tmp}/${asset}"
dl "${base}/${asset}.sha256" "${tmp}/${asset}.sha256"

# Verify checksum (sha256sum on Linux, shasum on macOS).
printf 'Verifying checksum ...\n'
expected="$(awk '{print $1}' "${tmp}/${asset}.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${tmp}/${asset}" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "${tmp}/${asset}" | awk '{print $1}')"
else
  err "need sha256sum or shasum to verify download"
fi
[ "$expected" = "$actual" ] || err "checksum mismatch (expected $expected, got $actual)"

printf 'Installing to %s ...\n' "$INSTALL_DIR"
tar -xzf "${tmp}/${asset}" -C "$tmp"
mkdir -p "$INSTALL_DIR"
mv "${tmp}/kamaji" "${INSTALL_DIR}/kamaji"
chmod +x "${INSTALL_DIR}/kamaji"

printf 'Installed: '
"${INSTALL_DIR}/kamaji" --version || true

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf '\nNote: %s is not on your PATH. Add it, e.g.:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR" "$INSTALL_DIR" ;;
esac
