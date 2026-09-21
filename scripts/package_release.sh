#!/usr/bin/env bash
#
# Packages a CrabBoy Advance Linux release: builds the release binary (if not
# already built), assembles the portable tarball, and computes real SHA-256
# checksums for every asset into SHA256SUMS.txt.
#
# The updater (src/ui/updater.rs) verifies downloads against SHA256SUMS.txt
# before ever installing them, so it must ship with every release. The asset
# names produced here must match updater.rs's EXE_ASSET_NAME for Linux.
#
#   ./scripts/package_release.sh 0.4.0
#
# Mirrors scripts/package_release.ps1 (Windows) and emits the same JSON summary.

set -euo pipefail

if [[ $# -lt 1 ]]; then
    echo "usage: $0 <version>   (e.g. $0 0.4.0)" >&2
    exit 2
fi
VERSION="$1"

# Anchor all relative paths to the repo root regardless of the caller's cwd.
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BIN_PATH="target/release/crabboy-advance"
# Must match EXE_ASSET_NAME for target_os="linux" in src/ui/updater.rs.
ASSET_NAME="crabboy-advance-linux-x86_64"
PKG_DIR="target/package_v${VERSION}_linux"
TAR_NAME="crabboy-advance-v${VERSION}-linux-x86_64.tar.gz"
TAR_PATH="target/$TAR_NAME"
SUMS_PATH="target/SHA256SUMS.txt"

if [[ ! -x "$BIN_PATH" ]]; then
    echo "Release binary not found at $BIN_PATH; building..." >&2
    cargo build --release
fi

rm -rf "$PKG_DIR"
mkdir -p "$PKG_DIR"

# Ship the binary under its release asset name so that what a user downloads
# directly and what the in-app updater fetches are the same artifact.
install -m755 "$BIN_PATH"                       "$PKG_DIR/$ASSET_NAME"
install -m644 assets/guide/manual.pdf           "$PKG_DIR/CrabBoy_Advance_Trainers_Guide.pdf"
install -m644 README.md                         "$PKG_DIR/README.md"
install -m644 LICENSE                           "$PKG_DIR/LICENSE"

# Include the desktop-integration files so a tarball user can register the app.
install -Dm644 packaging/linux/io.github.ssilkdev.CrabBoyAdvance.desktop \
    "$PKG_DIR/packaging/linux/io.github.ssilkdev.CrabBoyAdvance.desktop"
install -Dm644 packaging/linux/io.github.ssilkdev.CrabBoyAdvance.xml \
    "$PKG_DIR/packaging/linux/io.github.ssilkdev.CrabBoyAdvance.xml"
install -Dm755 packaging/linux/install.sh       "$PKG_DIR/packaging/linux/install.sh"
install -Dm644 assets/icon_256.png              "$PKG_DIR/assets/icon_256.png"

rm -f "$TAR_PATH"
# --sort=name + fixed mtime/owner make the tarball byte-reproducible, so the
# published checksum is stable across rebuilds of identical inputs.
tar --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@${SOURCE_DATE_EPOCH:-0}" \
    -czf "$TAR_PATH" -C "$PKG_DIR" .

sha_of() { sha256sum "$1" | awk '{print $1}'; }

BIN_HASH="$(sha_of "$BIN_PATH")"
TAR_HASH="$(sha_of "$TAR_PATH")"
PDF_HASH="$(sha_of assets/guide/manual.pdf)"
# Hash the icon that is actually SHIPPED in the tarball (assets/icon_256.png),
# not the large source assets/icon.png. Listing a file the release does not
# contain makes that SHA256SUMS.txt entry unverifiable by updater.rs.
ICON_HASH="$(sha_of assets/icon_256.png)"

# SHA256SUMS.txt format: "<hex digest>  <filename>" per line (sha256sum
# convention; parsed by updater.rs's find_checksum()).
cat > "$SUMS_PATH" <<EOF
$BIN_HASH  $ASSET_NAME
$TAR_HASH  $TAR_NAME
$PDF_HASH  CrabBoy_Advance_Trainers_Guide.pdf
$ICON_HASH  icon_256.png
EOF

echo "BIN Hash:  $BIN_HASH" >&2
echo "TAR Hash:  $TAR_HASH" >&2
echo "Wrote checksums to $SUMS_PATH" >&2

cat <<EOF
{
  "bin_hash": "$BIN_HASH",
  "tar_hash": "$TAR_HASH",
  "pdf_hash": "$PDF_HASH",
  "icon_hash": "$ICON_HASH",
  "tar_path": "$TAR_PATH",
  "tar_name": "$TAR_NAME",
  "asset_name": "$ASSET_NAME",
  "sums_path": "$SUMS_PATH"
}
EOF
