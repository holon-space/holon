#!/usr/bin/env bash
set -euo pipefail

# Build and optionally install the Holon Android APK.
# Usage: ./build-apk.sh [debug|release] [--install] [--launch]

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/../../.."

# Source .env for build environment
if [ -f "$SCRIPT_DIR/.env" ]; then
    set -a
    source "$SCRIPT_DIR/.env"
    set +a
fi

PROFILE="${1:-release}"
shift || true
INSTALL=false
LAUNCH=false
for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=true ;;
        --launch)  INSTALL=true; LAUNCH=true ;;
    esac
done

SDK="${ANDROID_SDK_HOME:-$HOME/Library/Android/sdk}"
BT="$SDK/build-tools/36.0.0"
PLATFORM="$SDK/platforms/android-36/android.jar"
BUILD="$SCRIPT_DIR/build"
MANIFEST="$SCRIPT_DIR/AndroidManifest.xml"
NDK_VER="29.0.14206865"
NDK="$SDK/ndk/$NDK_VER"

if [ "$PROFILE" = "debug" ]; then
    SO_DIR="$PROJECT_ROOT/target/aarch64-linux-android/debug"
else
    SO_DIR="$PROJECT_ROOT/target/aarch64-linux-android/release"
fi

SO="$SO_DIR/libholon_gpui.so"
if [ ! -f "$SO" ]; then
    echo "ERROR: $SO not found. Run from project root:"
    echo "  cd $PROJECT_ROOT"
    echo "  source frontends/gpui/android/.env"
    echo "  cargo ndk -t arm64-v8a -P 33 build -p holon-gpui --no-default-features --features mobile --lib --release"
    exit 1
fi

echo "Building APK from $SO ($(du -h "$SO" | cut -f1) $(file -b "$SO" | cut -d, -f1-2))"

rm -rf "$BUILD"
mkdir -p "$BUILD/lib/arm64-v8a"

cp "$SO" "$BUILD/lib/arm64-v8a/"
cp "$NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" \
   "$BUILD/lib/arm64-v8a/"

# Link manifest
"$BT/aapt2" link -o "$BUILD/holon-unaligned.apk" \
    --manifest "$MANIFEST" -I "$PLATFORM" \
    --min-sdk-version 33 --target-sdk-version 36

# Add native libs (stored, not compressed)
cd "$BUILD"
zip -0 holon-unaligned.apk lib/arm64-v8a/libholon_gpui.so lib/arm64-v8a/libc++_shared.so
cd "$SCRIPT_DIR"

# Align
"$BT/zipalign" -f 4 "$BUILD/holon-unaligned.apk" "$BUILD/holon.apk"

# Sign with debug keystore
"$BT/apksigner" sign --ks ~/.android/debug.keystore \
    --ks-pass pass:android --ks-key-alias androiddebugkey \
    "$BUILD/holon.apk"

echo ""
echo "APK ready: $BUILD/holon.apk ($(du -h "$BUILD/holon.apk" | cut -f1))"

if $INSTALL; then
    adb install -r "$BUILD/holon.apk"
fi
if $LAUNCH; then
    adb shell am start -n space.holon.gpui/android.app.NativeActivity
fi

if ! $INSTALL; then
    echo "Install:   adb install -r $BUILD/holon.apk"
    echo "Launch:    adb shell am start -n space.holon.gpui/android.app.NativeActivity"
fi
