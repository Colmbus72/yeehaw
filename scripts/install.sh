#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SRC="$ROOT/target/release/yeehaw"
BIN_DIR="$HOME/.local/bin"
BIN_DST="$BIN_DIR/yeehaw"
TMP="$BIN_DIR/.yeehaw.new"

mkdir -p "$BIN_DIR"

# Build release
echo "Building release..."
cd "$ROOT"
cargo build --release

# Copy to temp file (same filesystem as final destination → atomic mv)
cp "$BIN_SRC" "$TMP"

# Strip quarantine xattr if present (harmless if missing)
xattr -d com.apple.quarantine "$TMP" 2>/dev/null || true

# Ad-hoc codesign for local dev (avoids syspolicyd issues)
codesign --force --sign - "$TMP"

# Atomic replace — mv on same filesystem is rename(2), no partial reads
mv -f "$TMP" "$BIN_DST"

echo "Installed → $BIN_DST"
