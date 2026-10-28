#!/usr/bin/env bash
# Folio installer for Linux.
#
#   ./install.sh                  build from source, install for the current user
#   ./install.sh --remove-source  ...then delete this source folder (leaves nothing behind)
#   ./install.sh --yes            don't ask questions (installs missing build tools with sudo)
#
# Inside a release download (a folder that already contains the `folio` binary) this script
# skips the build and just installs. Everything lands in your home directory; no root needed
# except, optionally, for system build libraries.
set -euo pipefail

PDFIUM_TAG="chromium/7543"        # pinned: must match the pdfium-render API level in Cargo.toml
APP_ID="folio"
SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ASSUME_YES=0
REMOVE_SOURCE=0
DESKTOP_ICON=1
for arg in "$@"; do
  case "$arg" in
    -y|--yes) ASSUME_YES=1 ;;
    --remove-source) REMOVE_SOURCE=1 ;;
    --no-desktop-shortcut) DESKTOP_ICON=0 ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done

say()  { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31mxx\033[0m %s\n' "$*" >&2; exit 1; }
ask()  { # ask "question"  -> returns 0 for yes
  [ "$ASSUME_YES" = 1 ] && return 0
  read -r -p "$1 [Y/n] " reply </dev/tty || return 1
  [[ -z "$reply" || "$reply" =~ ^[Yy] ]]
}

[ "$(uname -s)" = "Linux" ] || die "This installer is for Linux. On Windows use install.ps1."

case "$(uname -m)" in
  x86_64|amd64)  PDFIUM_ARCH="x64" ;;
  aarch64|arm64) PDFIUM_ARCH="arm64" ;;
  *) die "Unsupported CPU architecture: $(uname -m)" ;;
esac

INSTALL_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/$APP_ID"
BIN_DIR="$HOME/.local/bin"
APPS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"

# ─── 1. Work out where the program files come from ───────────────────────────
if [ -x "$SELF_DIR/folio" ] && [ -f "$SELF_DIR/libpdfium.so" ]; then
  MODE="package"
  BIN_SRC="$SELF_DIR/folio"
  PDFIUM_SRC="$SELF_DIR/libpdfium.so"
  ICON_SRC_DIR="$SELF_DIR/icons"
  say "Installing from a prebuilt package."
else
  MODE="source"
  [ -f "$SELF_DIR/Cargo.toml" ] && [ -d "$SELF_DIR/src-tauri" ] || die "Run this script from the Folio source folder."
  cd "$SELF_DIR"
  BIN_SRC="$SELF_DIR/target/release/folio"
  PDFIUM_SRC="$SELF_DIR/vendor/pdfium/lib/libpdfium.so"
  ICON_SRC_DIR="$SELF_DIR/src-tauri/icons"
fi

# ─── 2. Build prerequisites (source installs only) ───────────────────────────
if [ "$MODE" = "source" ]; then
  missing_libs=0
  for pc in webkit2gtk-4.1 gtk+-3.0 librsvg-2.0 openssl; do
    pkg-config --exists "$pc" 2>/dev/null || missing_libs=1
  done
  command -v cc >/dev/null 2>&1 || missing_libs=1

  if [ "$missing_libs" = 1 ]; then
    warn "Some system libraries needed to build Folio are missing."
    if   command -v apt-get >/dev/null; then
      CMD="sudo apt-get update && sudo apt-get install -y build-essential pkg-config curl libwebkit2gtk-4.1-dev libssl-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev"
    elif command -v dnf >/dev/null; then
      CMD="sudo dnf install -y gcc gcc-c++ pkgconf-pkg-config curl webkit2gtk4.1-devel openssl-devel libxdo-devel libappindicator-gtk3-devel librsvg2-devel"
    elif command -v pacman >/dev/null; then
      CMD="sudo pacman -S --needed --noconfirm base-devel curl webkit2gtk-4.1 openssl xdotool libayatana-appindicator librsvg"
    elif command -v zypper >/dev/null; then
      CMD="sudo zypper install -y gcc gcc-c++ pkg-config curl webkit2gtk3-devel libopenssl-devel xdotool-devel libappindicator3-devel librsvg-devel"
    else
      die "Unknown package manager. Install the Tauri v2 Linux prerequisites (https://v2.tauri.app/start/prerequisites/) and re-run."
    fi
    echo "    $CMD"
    ask "Run that command now?" || die "Cannot build without those libraries."
    bash -c "$CMD"
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    warn "Rust is not installed."
    ask "Install Rust with rustup (into ~/.cargo, minimal profile)?" || die "Rust is required to build from source."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    . "$HOME/.cargo/env"
  fi

  # ─── 3. PDFium (the rendering engine) ──────────────────────────────────────
  if [ ! -f "$PDFIUM_SRC" ]; then
    say "Downloading PDFium ($PDFIUM_TAG, linux-$PDFIUM_ARCH)..."
    mkdir -p vendor/pdfium
    url="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_TAG}/pdfium-linux-${PDFIUM_ARCH}.tgz"
    curl -fL --retry 3 -o vendor/pdfium.tgz "$url" || die "Could not download PDFium from $url"
    tar -xzf vendor/pdfium.tgz -C vendor/pdfium
    rm -f vendor/pdfium.tgz
  fi
  [ -f "$PDFIUM_SRC" ] || die "PDFium library missing after download."

  # ─── 4. Build ──────────────────────────────────────────────────────────────
  say "Building Folio (first build takes a few minutes)..."
  cargo build --release -p folio
fi

[ -x "$BIN_SRC" ] || die "Build output not found: $BIN_SRC"

# ─── 5. Install ──────────────────────────────────────────────────────────────
say "Installing to $INSTALL_DIR"
pkill -x folio 2>/dev/null || true
mkdir -p "$INSTALL_DIR" "$BIN_DIR" "$APPS_DIR"
install -m 755 "$BIN_SRC" "$INSTALL_DIR/folio"
install -m 644 "$PDFIUM_SRC" "$INSTALL_DIR/libpdfium.so"
for f in LICENSE THIRD-PARTY-NOTICES.md; do [ -f "$SELF_DIR/$f" ] && install -m 644 "$SELF_DIR/$f" "$INSTALL_DIR/$f"; done
install -m 755 "$SELF_DIR/uninstall.sh" "$INSTALL_DIR/uninstall.sh"
ln -sf "$INSTALL_DIR/folio" "$BIN_DIR/folio"

# Icons for every size the desktop might ask for.
if [ -d "$ICON_SRC_DIR" ]; then
  for pair in "32x32:32x32.png" "128x128:128x128.png" "256x256:128x128@2x.png" "512x512:icon.png"; do
    size="${pair%%:*}"; file="${pair#*:}"
    if [ -f "$ICON_SRC_DIR/$file" ]; then
      mkdir -p "$ICONS_DIR/$size/apps"
      install -m 644 "$ICON_SRC_DIR/$file" "$ICONS_DIR/$size/apps/$APP_ID.png"
    fi
  done
fi

cat > "$APPS_DIR/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=Folio
GenericName=PDF Viewer and Editor
Comment=View, annotate, rearrange, merge and split PDF files
Exec="$INSTALL_DIR/folio" %F
Icon=$APP_ID
Terminal=false
Categories=Office;Viewer;Graphics;
MimeType=application/pdf;
StartupWMClass=folio
EOF
chmod 644 "$APPS_DIR/$APP_ID.desktop"

if [ "$DESKTOP_ICON" = 1 ]; then
  DESKTOP_DIR="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
  if [ -d "$DESKTOP_DIR" ]; then
    cp "$APPS_DIR/$APP_ID.desktop" "$DESKTOP_DIR/Folio.desktop"
    chmod +x "$DESKTOP_DIR/Folio.desktop"
    gio set "$DESKTOP_DIR/Folio.desktop" metadata::trusted true 2>/dev/null || true
  fi
fi

command -v update-desktop-database >/dev/null && update-desktop-database "$APPS_DIR" 2>/dev/null || true
command -v gtk-update-icon-cache   >/dev/null && gtk-update-icon-cache -q -t "$ICONS_DIR" 2>/dev/null || true

say "Folio is installed. Find it in your applications menu, or run: folio"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) warn "$BIN_DIR is not on your PATH; use the menu entry or add it to PATH to type 'folio'." ;; esac
echo "    To make Folio the default PDF app: right-click a PDF > Open With > Folio > Always."

# ─── 6. Optional clean-up of the source folder ───────────────────────────────
if [ "$MODE" = "source" ]; then
  if [ "$REMOVE_SOURCE" = 0 ] && [ "$ASSUME_YES" = 0 ]; then
    ask "Delete this source folder ($SELF_DIR) now? Folio keeps working without it." && REMOVE_SOURCE=1 || true
  fi
  if [ "$REMOVE_SOURCE" = 1 ]; then
    cd "$HOME"
    rm -rf "$SELF_DIR"
    say "Source folder removed. (Rust itself stays in ~/.cargo and ~/.rustup; remove with 'rustup self uninstall' if you don't need it.)"
  fi
fi
