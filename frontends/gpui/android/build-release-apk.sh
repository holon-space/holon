#!/usr/bin/env bash
set -euo pipefail

# Package a RELEASE-signed Holon APK. Companion to build-apk.sh (the dev/debug
# path); this one never touches the debug keystore. The signing keystore and
# version identity come exclusively from the environment — every variable is
# required and the script fails loud if any is missing.
#
# Required env:
#   KEYSTORE_FILE       path to the release .jks/.keystore
#   KEYSTORE_PASSWORD   keystore (store) password
#   KEY_ALIAS           key alias inside the keystore
#   KEY_PASSWORD        key password
#   VERSION_NAME        e.g. 1.2.0
#   VERSION_CODE        monotonically increasing integer (Play requirement)
# Optional env:
#   ANDROID_SDK_HOME / ANDROID_HOME   SDK root (one must be set)
#   ANDROID_NDK_HOME                  NDK root (defaults to the pinned version)
#
# Prerequisite: the release .so must already exist —
#   cargo ndk -t arm64-v8a -P 33 build -p holon-gpui \
#     --no-default-features --features mobile --lib --release

: "${KEYSTORE_FILE:?KEYSTORE_FILE must point at the release keystore}"
: "${KEYSTORE_PASSWORD:?KEYSTORE_PASSWORD is required}"
: "${KEY_ALIAS:?KEY_ALIAS is required}"
: "${KEY_PASSWORD:?KEY_PASSWORD is required}"
: "${VERSION_NAME:?VERSION_NAME is required (e.g. 1.2.0)}"
: "${VERSION_CODE:?VERSION_CODE is required (integer)}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/../../.."

SDK="${ANDROID_SDK_HOME:-${ANDROID_HOME:?ANDROID_SDK_HOME or ANDROID_HOME must be set}}"
BT="$SDK/build-tools/36.0.0"
PLATFORM="$SDK/platforms/android-36/android.jar"
NDK="${ANDROID_NDK_HOME:-$SDK/ndk/29.0.14206865}"
BUILD="$SCRIPT_DIR/build-release"
MANIFEST_SRC="$SCRIPT_DIR/AndroidManifest.xml"

case "$(uname -s)" in
    Darwin) NDK_HOST="darwin-x86_64" ;;
    Linux)  NDK_HOST="linux-x86_64" ;;
    *) echo "ERROR: unsupported host OS: $(uname -s)" >&2; exit 1 ;;
esac
LIBCXX="$NDK/toolchains/llvm/prebuilt/$NDK_HOST/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"

SO="$PROJECT_ROOT/target/aarch64-linux-android/release/libholon_gpui.so"
for f in "$SO" "$LIBCXX" "$PLATFORM" "$BT/aapt2" "$KEYSTORE_FILE"; do
    if [ ! -e "$f" ]; then
        echo "ERROR: required file missing: $f" >&2
        exit 1
    fi
done

rm -rf "$BUILD"
mkdir -p "$BUILD/lib/arm64-v8a"
cp "$SO" "$BUILD/lib/arm64-v8a/"
cp "$LIBCXX" "$BUILD/lib/arm64-v8a/"

# The checked-in manifest is a dev manifest (android:debuggable="true").
# Google Play rejects debuggable APKs, so strip the attribute for release —
# and fail loud if the manifest shape changed and the strip did nothing.
MANIFEST="$BUILD/AndroidManifest.xml"
sed 's/ android:debuggable="true"//' "$MANIFEST_SRC" > "$MANIFEST"
if grep -q 'debuggable' "$MANIFEST"; then
    echo "ERROR: failed to strip android:debuggable from $MANIFEST_SRC" >&2
    exit 1
fi
if cmp -s "$MANIFEST_SRC" "$MANIFEST"; then
    echo "ERROR: manifest unchanged — expected to strip android:debuggable=\"true\"" >&2
    exit 1
fi

"$BT/aapt2" link -o "$BUILD/holon-unaligned.apk" \
    --manifest "$MANIFEST" -I "$PLATFORM" \
    --min-sdk-version 33 --target-sdk-version 36 \
    --version-code "$VERSION_CODE" --version-name "$VERSION_NAME"

(cd "$BUILD" && zip -0 holon-unaligned.apk lib/arm64-v8a/libholon_gpui.so lib/arm64-v8a/libc++_shared.so)

"$BT/zipalign" -f 4 "$BUILD/holon-unaligned.apk" "$BUILD/holon-release.apk"

"$BT/apksigner" sign \
    --ks "$KEYSTORE_FILE" \
    --ks-pass "pass:$KEYSTORE_PASSWORD" \
    --ks-key-alias "$KEY_ALIAS" \
    --key-pass "pass:$KEY_PASSWORD" \
    "$BUILD/holon-release.apk"

"$BT/apksigner" verify "$BUILD/holon-release.apk"

echo "Release APK ready: $BUILD/holon-release.apk (versionName=$VERSION_NAME versionCode=$VERSION_CODE)"
