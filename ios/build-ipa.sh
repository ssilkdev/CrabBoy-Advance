#!/usr/bin/env bash
# Build CrabBoy Advance for iPhone / iPad on Linux (no Mac, no Xcode) and
# package an unsigned .ipa. Signing + install is done by xtool, which
# fetches a free-Apple-ID development certificate:
#
#   ios/build-ipa.sh                 -> target/ios/CrabBoyAdvance.ipa
#   ios/build-ipa.sh --install       -> ... and sign + install over USB
#
# Toolchain (see README.md in this folder): ~/ios/env.sh sets up an LLVM
# release + the theos iPhoneOS SDK + `rustup target add aarch64-apple-ios`.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
ENV_SH="${IOS_ENV:-$HOME/ios/env.sh}"
XTOOL="${XTOOL:-$HOME/ios/xtool.AppImage}"
[[ -f "$ENV_SH" ]] || { echo "missing $ENV_SH (iOS cross toolchain)"; exit 1; }
# shellcheck disable=SC1090
source "$ENV_SH"

VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
BUILD="$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 1)"

cargo build --manifest-path "$ROOT/android/Cargo.toml" --target aarch64-apple-ios --release --bin crabboy-ios

OUT="$ROOT/target/ios"
APP="$OUT/Payload/CrabBoy.app"
rm -rf "$OUT/Payload" && mkdir -p "$APP"
cp "$ROOT/android/target/aarch64-apple-ios/release/crabboy-ios" "$APP/crabboy-ios"
sed -e "s/@VERSION@/$VERSION/" -e "s/@BUILD@/$BUILD/" "$HERE/Info.plist" > "$APP/Info.plist"
printf 'APPL????' > "$APP/PkgInfo"

# App icons from assets/icon.png (iOS rejects alpha in app icons).
python3 - "$ROOT/assets/icon.png" "$APP" <<'EOF'
import sys
from PIL import Image
src, app = sys.argv[1], sys.argv[2]
im = Image.open(src).convert("RGBA")
bg = Image.new("RGBA", im.size, (24, 24, 32, 255))
bg.alpha_composite(im)
im = bg.convert("RGB")
for name, px in [("AppIcon60x60@2x", 120), ("AppIcon60x60@3x", 180), ("AppIcon76x76", 76),
                 ("AppIcon76x76@2x", 152), ("AppIcon83.5x83.5@2x", 167)]:
    suffix = "~ipad" if "76" in name or "83" in name else ""
    im.resize((px, px), Image.LANCZOS).save(f"{app}/{name}{suffix}.png")
EOF

(cd "$OUT" && rm -f CrabBoyAdvance.ipa && zip -qr CrabBoyAdvance.ipa Payload)
echo "Built $OUT/CrabBoyAdvance.ipa ($VERSION build $BUILD)"

if [[ "${1:-}" == "--install" ]]; then
    "$XTOOL" install --usb "$OUT/CrabBoyAdvance.ipa"
fi
