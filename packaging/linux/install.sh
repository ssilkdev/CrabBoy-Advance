#!/usr/bin/env bash
#
# Installs CrabBoy Advance into a standard XDG location.
#
#   ./packaging/linux/install.sh            # per-user install into ~/.local
#   sudo ./packaging/linux/install.sh --system   # system-wide into /usr/local
#   ./packaging/linux/install.sh --uninstall
#
# A per-user install needs no root. It does require ~/.local/bin on PATH,
# which is the systemd/XDG default on modern distros but is checked below.

set -euo pipefail

APP_ID="io.github.ssilkdev.CrabBoyAdvance"
BIN_NAME="crabboy-advance"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

MODE="user"
ACTION="install"
for arg in "$@"; do
    case "$arg" in
        --system)    MODE="system" ;;
        --user)      MODE="user" ;;
        --uninstall) ACTION="uninstall" ;;
        -h|--help)   sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "Unknown option: $arg" >&2; exit 2 ;;
    esac
done

if [[ "$MODE" == "system" ]]; then
    PREFIX="/usr/local"
    DATA_DIR="$PREFIX/share"
    if [[ $EUID -ne 0 ]]; then
        echo "error: --system requires root. Re-run with sudo." >&2
        exit 1
    fi
else
    PREFIX="$HOME/.local"
    DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
fi

BIN_DIR="$PREFIX/bin"
DESKTOP_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor/256x256/apps"
MIME_DIR="$DATA_DIR/mime/packages"

# Refresh the desktop/MIME/icon caches. Each tool is optional: a minimal or
# headless system may not ship them, and a missing cache is not fatal -- the
# files are installed correctly either way.
refresh_caches() {
    command -v update-desktop-database >/dev/null 2>&1 \
        && update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
    command -v update-mime-database >/dev/null 2>&1 \
        && update-mime-database "$DATA_DIR/mime" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 \
        && gtk-update-icon-cache -qtf "$DATA_DIR/icons/hicolor" 2>/dev/null || true
}

if [[ "$ACTION" == "uninstall" ]]; then
    rm -f "$BIN_DIR/$BIN_NAME" \
          "$DESKTOP_DIR/$APP_ID.desktop" \
          "$ICON_DIR/$APP_ID.png" \
          "$MIME_DIR/$APP_ID.xml"
    refresh_caches
    echo "Uninstalled CrabBoy Advance from $PREFIX."
    exit 0
fi

BINARY="$REPO_ROOT/target/release/$BIN_NAME"
if [[ ! -x "$BINARY" ]]; then
    echo "error: release binary not found at $BINARY" >&2
    echo "Build it first:  cargo build --release" >&2
    exit 1
fi

install -Dm755 "$BINARY"                                      "$BIN_DIR/$BIN_NAME"
install -Dm644 "$REPO_ROOT/assets/icon_256.png"               "$ICON_DIR/$APP_ID.png"
install -Dm644 "$REPO_ROOT/packaging/linux/$APP_ID.desktop"   "$DESKTOP_DIR/$APP_ID.desktop"
install -Dm644 "$REPO_ROOT/packaging/linux/$APP_ID.xml"       "$MIME_DIR/$APP_ID.xml"

refresh_caches

echo "Installed CrabBoy Advance:"
echo "  binary   -> $BIN_DIR/$BIN_NAME"
echo "  desktop  -> $DESKTOP_DIR/$APP_ID.desktop"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo
       echo "warning: $BIN_DIR is not on your PATH."
       echo "  Add it with:  echo 'export PATH=\"\$PATH:$BIN_DIR\"' >> ~/.bashrc" ;;
esac
