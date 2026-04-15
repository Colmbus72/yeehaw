#!/bin/sh
# Yeehaw CLI installer
# Usage: curl -LsSf https://yeehaw.cool/install.sh | sh

set -e

REPO="Colmbus72/yeehaw"
BIN="yeehaw"
INSTALL_DIR="${YEEHAW_INSTALL_DIR:-$HOME/.local/bin}"

# ---- helpers ------------------------------------------------------------

err() { printf "\033[31merror:\033[0m %s\n" "$*" >&2; exit 1; }
info() { printf "\033[36m%s\033[0m %s\n" "::" "$*"; }
ok() { printf "\033[32m%s\033[0m %s\n" "✓" "$*"; }

have() { command -v "$1" >/dev/null 2>&1; }

# ---- detect target ------------------------------------------------------

kernel="$(uname -s)"
case "$kernel" in
  Darwin) os_slug="apple-darwin" ;;
  Linux)  os_slug="unknown-linux-gnu" ;;
  *)      err "unsupported OS: $kernel (supported: macOS, Linux)" ;;
esac

machine="$(uname -m)"
case "$machine" in
  arm64|aarch64) arch_slug="aarch64" ;;
  x86_64|amd64)  arch_slug="x86_64" ;;
  *)             err "unsupported arch: $machine (supported: x86_64, arm64)" ;;
esac

target="${arch_slug}-${os_slug}"
asset="${BIN}-${target}.tar.xz"
url="https://github.com/${REPO}/releases/latest/download/${asset}"

info "installing ${BIN} for ${target}"

# ---- download & extract -------------------------------------------------

have curl || err "curl is required"
have tar  || err "tar is required"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT HUP TERM

info "downloading ${url}"
if ! curl -fLSs -o "$tmp/$asset" "$url"; then
  err "download failed — the release may not exist yet, or your platform has no prebuilt binary"
fi

info "extracting"
tar -xf "$tmp/$asset" -C "$tmp" || err "extraction failed"

# tarball contains a single top-level directory named after the target
extracted_bin="$tmp/${BIN}-${target}/${BIN}"
[ -f "$extracted_bin" ] || err "binary not found in archive at expected path"

# ---- install ------------------------------------------------------------

mkdir -p "$INSTALL_DIR"
dest="$INSTALL_DIR/$BIN"

# Atomic replace via same-filesystem rename
staged="$INSTALL_DIR/.${BIN}.new"
cp "$extracted_bin" "$staged"
chmod +x "$staged"
mv -f "$staged" "$dest"

# Remove the quarantine flag if the tarball came from a browser download path.
# (curl-piped sh won't set it, but this covers manual download users too.)
if [ "$kernel" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$dest" 2>/dev/null || true
fi

ok "installed ${BIN} → ${dest}"

# ---- PATH check ---------------------------------------------------------

case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    ok "${INSTALL_DIR} is on your PATH"
    ;;
  *)
    printf "\n"
    printf "\033[33mNote:\033[0m %s is not on your PATH.\n" "$INSTALL_DIR"
    printf "  Add this to your shell rc file (e.g. ~/.zshrc or ~/.bashrc):\n\n"
    printf "    export PATH=\"%s:\$PATH\"\n\n" "$INSTALL_DIR"
    ;;
esac

printf "\n"
printf "Run \033[1m%s\033[0m to get started.\n" "$BIN"
