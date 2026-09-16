#!/usr/bin/env bash
# Builds a release binary and installs it, its .desktop entry, and its icon.
#
# Usage:
#   packaging/install.sh              # user-level install, no sudo (~/.local/*)
#   packaging/install.sh --system     # system-wide install (/usr/local/*), uses sudo
#   packaging/install.sh --uninstall           # remove a user-level install
#   packaging/install.sh --uninstall --system  # remove a system-wide install
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME=sgithi-audio-meter

SYSTEM=0
UNINSTALL=0
for arg in "$@"; do
  case "$arg" in
    --system) SYSTEM=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help)
      sed -n '2,8p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

if [ "$SYSTEM" -eq 1 ]; then
  BIN_DIR=/usr/local/bin
  DESKTOP_DIR=/usr/local/share/applications
  ICON_DIR=/usr/local/share/icons/hicolor/scalable/apps
  SUDO=sudo
else
  BIN_DIR="$HOME/.local/bin"
  DESKTOP_DIR="$HOME/.local/share/applications"
  ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
  SUDO=""
fi

BIN_PATH="$BIN_DIR/$APP_NAME"
DESKTOP_PATH="$DESKTOP_DIR/$APP_NAME.desktop"
ICON_PATH="$ICON_DIR/$APP_NAME.svg"

if [ "$UNINSTALL" -eq 1 ]; then
  echo "Removing $BIN_PATH, $DESKTOP_PATH, $ICON_PATH"
  $SUDO rm -f "$BIN_PATH" "$DESKTOP_PATH" "$ICON_PATH"
  command -v update-desktop-database >/dev/null 2>&1 && $SUDO update-desktop-database "$DESKTOP_DIR" || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && $SUDO gtk-update-icon-cache -f -t "$(dirname "$(dirname "$ICON_DIR")")" 2>/dev/null || true
  echo "Uninstalled."
  exit 0
fi

# A downloaded release tarball ships a pre-built binary next to this script (see
# .github/workflows/release.yml); a git checkout doesn't, so build one from source instead.
if [ -x "$REPO_ROOT/$APP_NAME" ]; then
  SRC_BIN="$REPO_ROOT/$APP_NAME"
else
  echo "Building release binary..."
  (cd "$REPO_ROOT" && cargo build --release)
  SRC_BIN="$REPO_ROOT/target/release/$APP_NAME"
fi

echo "Installing to $BIN_DIR, $DESKTOP_DIR, $ICON_DIR"
$SUDO mkdir -p "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"
$SUDO install -m 755 "$SRC_BIN" "$BIN_PATH"
# The .desktop template's Exec is a placeholder - it has to be the absolute install path (not
# just the binary name) since a user-level install to ~/.local/bin isn't guaranteed to be on
# every desktop environment's PATH when it launches an app from the .desktop entry.
sed "s|@BIN_PATH@|$BIN_PATH|" "$REPO_ROOT/packaging/$APP_NAME.desktop" | $SUDO tee "$DESKTOP_PATH" >/dev/null
$SUDO chmod 644 "$DESKTOP_PATH"
$SUDO install -m 644 "$REPO_ROOT/packaging/icons/$APP_NAME.svg" "$ICON_PATH"

command -v update-desktop-database >/dev/null 2>&1 && $SUDO update-desktop-database "$DESKTOP_DIR" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && $SUDO gtk-update-icon-cache -f -t "$(dirname "$(dirname "$ICON_DIR")")" 2>/dev/null || true

echo "Installed. Run with '$APP_NAME' or find \"Sgithi Audio Meter\" in your application launcher."
if [ "$SYSTEM" -eq 0 ] && ! echo "$PATH" | tr ':' '\n' | grep -qx "$BIN_DIR"; then
  echo "Note: $BIN_DIR isn't on your PATH - add it to your shell profile to run '$APP_NAME' directly."
fi
