#!/usr/bin/env bash
# Build the CrabBoy Advance Android APK without Gradle.
#
#   android/build-apk.sh [--abi arm64-v8a,x86_64] [--debug]
#
# Requires: JDK 17+, Android SDK (platforms;android-35, build-tools;35.0.1),
# NDK r27+, `cargo install cargo-ndk`, and the Rust targets
# aarch64-linux-android / x86_64-linux-android.
# Env: ANDROID_HOME (or ANDROID_SDK_ROOT), optional ANDROID_NDK_HOME, JAVA_HOME.
# Signing: uses $CRABBOY_KEYSTORE (+ $CRABBOY_KEYSTORE_PASS) if set, otherwise
# a local debug keystore that is created on first run.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ABIS="arm64-v8a"
PROFILE="release"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --abi) ABIS="$2"; shift 2 ;;
        --debug) PROFILE="debug"; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[[ -d "$SDK" ]] || { echo "Set ANDROID_HOME to your Android SDK" >&2; exit 1; }
PLATFORM="$SDK/platforms/android-35/android.jar"
BT="$SDK/build-tools/35.0.1"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d "$SDK"/ndk/* | sort -V | tail -1)}"
for f in "$PLATFORM" "$BT/aapt2" "$BT/d8" "$BT/apksigner" "$BT/zipalign"; do
    [[ -e "$f" ]] || { echo "Missing $f -- install platforms;android-35 and build-tools;35.0.1" >&2; exit 1; }
done

OUT="$HERE/build"
rm -rf "$OUT"
mkdir -p "$OUT"/{classes,dex,apk/lib}

# 1. Native library, one per ABI.
cargo_args=(build --manifest-path "$HERE/Cargo.toml")
[[ "$PROFILE" == release ]] && cargo_args+=(--release)
ndk_targets=()
IFS=',' read -ra abi_list <<< "$ABIS"
for abi in "${abi_list[@]}"; do ndk_targets+=(-t "$abi"); done
echo "==> Building native library ($ABIS, $PROFILE)"
cargo ndk "${ndk_targets[@]}" --platform 26 -o "$OUT/apk/lib" "${cargo_args[@]}"

# 2. Java activity -> dex.
echo "==> Compiling Java"
javac --release 11 -Xlint:-options -classpath "$PLATFORM" -d "$OUT/classes" \
    $(find "$HERE/java" -name '*.java')
"$BT/d8" --min-api 26 --lib "$PLATFORM" --output "$OUT/dex" $(find "$OUT/classes" -name '*.class')
cp "$OUT/dex/classes.dex" "$OUT/apk/"

# 3. Resources + manifest.
echo "==> Packaging"
"$BT/aapt2" compile --dir "$HERE/res" -o "$OUT/res.zip"
"$BT/aapt2" link -o "$OUT/base.apk" -I "$PLATFORM" \
    --manifest "$HERE/AndroidManifest.xml" "$OUT/res.zip"
(cd "$OUT/apk" && zip -qr "$OUT/base.apk" classes.dex lib)

# 4. Align (native libs stored uncompressed and page-aligned) and sign.
"$BT/zipalign" -P 16 -f 4 "$OUT/base.apk" "$OUT/aligned.apk"
KS="${CRABBOY_KEYSTORE:-$HERE/debug.keystore}"
PASS="${CRABBOY_KEYSTORE_PASS:-android}"
if [[ ! -f "$KS" ]]; then
    echo "==> Creating debug keystore $KS"
    keytool -genkeypair -keystore "$KS" -storepass "$PASS" -keypass "$PASS" \
        -alias crabboy -keyalg RSA -keysize 2048 -validity 10000 \
        -dname "CN=CrabBoy Advance Debug" >/dev/null
fi
APK="$OUT/crabboy-advance-$( [[ "$PROFILE" == release ]] && echo release || echo debug ).apk"
"$BT/apksigner" sign --ks "$KS" --ks-pass "pass:$PASS" --out "$APK" "$OUT/aligned.apk"
"$BT/apksigner" verify "$APK"
echo "==> $APK ($(du -h "$APK" | cut -f1))"
