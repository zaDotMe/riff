#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/512x512/apps"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"

install -m 755 "$SCRIPT_DIR/riff-slint" "$BIN_DIR/riff-slint"
if [ -f "$SCRIPT_DIR/riff-tauri" ]; then
  install -m 755 "$SCRIPT_DIR/riff-tauri" "$BIN_DIR/riff-tauri"
fi
if [ -f "$SCRIPT_DIR/riff-core" ]; then
  install -m 755 "$SCRIPT_DIR/riff-core" "$BIN_DIR/riff-core"
fi

install -m 644 "$SCRIPT_DIR/riff.png" "$ICON_DIR/riff.png"
install -m 644 "$SCRIPT_DIR/riff.desktop" "$APP_DIR/riff.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" || true
fi

echo "Installed Riff to $BIN_DIR (run: riff-slint)"
