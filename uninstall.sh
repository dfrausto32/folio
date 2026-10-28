#!/usr/bin/env bash
# Removes Folio (program files, menu entry, icons, desktop shortcut). Your PDFs are never touched.
set -euo pipefail

APP_ID="folio"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DESKTOP_DIR="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"

pkill -x folio 2>/dev/null || true
rm -f  "$HOME/.local/bin/folio"
rm -f  "$DATA/applications/$APP_ID.desktop"
rm -f  "$DESKTOP_DIR/Folio.desktop"
for size in 32x32 128x128 256x256 512x512; do rm -f "$DATA/icons/hicolor/$size/apps/$APP_ID.png"; done
command -v update-desktop-database >/dev/null && update-desktop-database "$DATA/applications" 2>/dev/null || true
command -v gtk-update-icon-cache   >/dev/null && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
# App data the web view created (saved theme and caches). The folder name is the app identifier.
for d in "$DATA" "${XDG_CACHE_HOME:-$HOME/.cache}" "${XDG_CONFIG_HOME:-$HOME/.config}"; do rm -rf "$d/io.github.folio.pdf"; done
# Remove the program folder last: this script may be running from inside it.
rm -rf "$DATA/$APP_ID"
echo "Folio has been removed."
