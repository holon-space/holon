#!/usr/bin/env bash
set -euo pipefail

# Package a RELEASE-signed Holon Android App Bundle (.aab). Companion to
# build-release-apk.sh — same keystore, same version identity, same manifest —
# but produces the AAB that Google Play REQUIRES for a new app (APK uploads are
# rejected: "APKs are not allowed for this application"). There is no Gradle
# project; this is a direct aapt2(--proto-format) + bundletool pipeline.
#
# Required env:
#   KEYSTORE_FILE       path to the release .jks/.keystore
#   KEYSTORE_PASSWORD   keystore (store) password
#   KEY_ALIAS           key alias inside the keystore
#   KEY_PASSWORD        key password
#   VERSION_NAME        e.g. 1.2.0
#   VERSION_CODE        monotonically increasing integer (Play requirement)
#   BUNDLETOOL_JAR      path to a bundletool-all-<version>.jar. CI downloads +
#                       checksum-verifies this before calling the script; local
#                       runs point it at a cached copy.
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
: "${BUNDLETOOL_JAR:?BUNDLETOOL_JAR must point at a bundletool-all-<version>.jar}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/../../.."

SDK="${ANDROID_SDK_HOME:-${ANDROID_HOME:?ANDROID_SDK_HOME or ANDROID_HOME must be set}}"
BT="$SDK/build-tools/36.0.0"
PLATFORM="$SDK/platforms/android-36/android.jar"
NDK="${ANDROID_NDK_HOME:-$SDK/ndk/29.0.14206865}"
BUILD="$SCRIPT_DIR/build-release-aab"
MANIFEST_SRC="$SCRIPT_DIR/AndroidManifest.xml"

case "$(uname -s)" in
    Darwin) NDK_HOST="darwin-x86_64" ;;
    Linux)  NDK_HOST="linux-x86_64" ;;
    *) echo "ERROR: unsupported host OS: $(uname -s)" >&2; exit 1 ;;
esac
LIBCXX="$NDK/toolchains/llvm/prebuilt/$NDK_HOST/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"

SO="$PROJECT_ROOT/target/aarch64-linux-android/release/libholon_gpui.so"
for f in "$SO" "$LIBCXX" "$PLATFORM" "$BT/aapt2" "$BT/d8" "$KEYSTORE_FILE" "$BUNDLETOOL_JAR"; do
    if [ ! -e "$f" ]; then
        echo "ERROR: required file missing: $f" >&2
        exit 1
    fi
done
command -v javac >/dev/null || { echo "ERROR: javac not on PATH (a JDK is required to build classes.dex)" >&2; exit 1; }

rm -rf "$BUILD"
mkdir -p "$BUILD/module/lib/arm64-v8a" "$BUILD/module/manifest" "$BUILD/module/dex" \
    "$BUILD/classes" "$BUILD/dex" "$BUILD/proto"
cp "$SO" "$BUILD/module/lib/arm64-v8a/"
cp "$LIBCXX" "$BUILD/module/lib/arm64-v8a/"

# Compile the vendored soft-keyboard host class (dev.gpui.mobile.GpuiTextInputView)
# to classes.dex. This app is NO LONGER a pure no-dex NativeActivity: the GPUI
# runtime looks this class up via find_app_class + JNI to raise the platform IME,
# so the dex is REQUIRED in the base module (manifest hasCode="true"). Small and
# self-contained (imports only android.*) — always rebuilt.
JAVA_SRC="$SCRIPT_DIR/java/dev/gpui/mobile/GpuiTextInputView.java"
javac -source 8 -target 8 -cp "$PLATFORM" -d "$BUILD/classes" "$JAVA_SRC"
"$BT/d8" --min-api 33 --output "$BUILD/dex" "$BUILD/classes"/dev/gpui/mobile/*.class
# bundletool base-module layout carries dex under dex/ (peer of manifest/, lib/).
cp "$BUILD/dex/classes.dex" "$BUILD/module/dex/classes.dex"

# The checked-in manifest is a dev manifest (android:debuggable="true").
# Google Play rejects debuggable builds, so strip the attribute for release —
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

# 1. Link the manifest (and any resources — there are none today) into a
#    PROTOBUF-format APK. bundletool consumes proto, not the binary AXML the
#    APK pipeline uses. No -R inputs: the app ships no resources, so aapt2 still
#    emits a (near-empty) resources.pb, which the base module requires.
"$BT/aapt2" link --proto-format -o "$BUILD/base-proto.apk" \
    --manifest "$MANIFEST" -I "$PLATFORM" \
    --min-sdk-version 33 --target-sdk-version 36 \
    --version-code "$VERSION_CODE" --version-name "$VERSION_NAME"

# 2. Unpack the proto APK → proto AndroidManifest.xml + resources.pb.
unzip -o -q "$BUILD/base-proto.apk" -d "$BUILD/proto"
for f in AndroidManifest.xml resources.pb; do
    if [ ! -e "$BUILD/proto/$f" ]; then
        echo "ERROR: aapt2 --proto-format did not emit $f" >&2
        exit 1
    fi
done

# 3. Assemble the base module in bundletool's expected layout:
#      manifest/AndroidManifest.xml   (proto)
#      dex/classes.dex                (the soft-keyboard host class)
#      resources.pb
#      lib/arm64-v8a/*.so
#    The dex is REQUIRED (manifest hasCode="true"): dropping it silently ships a
#    build where the keyboard can never surface. The bundle is asserted below.
cp "$BUILD/proto/AndroidManifest.xml" "$BUILD/module/manifest/AndroidManifest.xml"
cp "$BUILD/proto/resources.pb" "$BUILD/module/resources.pb"
(cd "$BUILD/module" && zip -q -r -X "../base.zip" manifest dex resources.pb lib)

# 4. Build the bundle.
java -jar "$BUNDLETOOL_JAR" build-bundle \
    --modules="$BUILD/base.zip" \
    --output="$BUILD/holon-release.aab"

# Assert the SHIPPED bundle carries the dex. The manifest declares hasCode="true",
# so a bundle without it installs fine and then fails the runtime find_app_class
# lookup — a defect only reproducible on-device. Checking the .aab itself (not the
# staging dir bundletool consumed) is what makes this assertion meaningful.
if ! unzip -Z1 "$BUILD/holon-release.aab" | grep -q 'dex/classes\.dex$'; then
    echo "ERROR: holon-release.aab has no dex/classes.dex — the soft-keyboard host" >&2
    echo "       class (dev.gpui.mobile.GpuiTextInputView) is missing from the bundle." >&2
    exit 1
fi

# 5. Sign the AAB with jarsigner (Play's requirement for uploaded bundles),
#    using the SAME keystore/env as the APK's apksigner step. Passwords are
#    passed via :env so they never appear in the process list or logs.
jarsigner -keystore "$KEYSTORE_FILE" \
    -storepass:env KEYSTORE_PASSWORD \
    -keypass:env KEY_PASSWORD \
    -sigalg SHA256withRSA -digestalg SHA-256 \
    "$BUILD/holon-release.aab" "$KEY_ALIAS"

jarsigner -verify "$BUILD/holon-release.aab"

# 6. Structural validation.
java -jar "$BUNDLETOOL_JAR" validate --bundle="$BUILD/holon-release.aab"

echo "Release AAB ready: $BUILD/holon-release.aab (versionName=$VERSION_NAME versionCode=$VERSION_CODE)"
