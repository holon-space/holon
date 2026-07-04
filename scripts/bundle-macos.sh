#!/usr/bin/env bash
set -euo pipefail

# Assemble Holon.app from a built holon-gpui binary.
# Usage: bundle-macos.sh <path-to-holon-gpui-binary> <output-dir>
#
# Mirrors `just bundle` (frontends/gpui/justfile) but takes the binary path as
# an argument so CI can pass a lipo'd universal binary. Assets land in
# Contents/Resources/assets with a symlink from Contents/MacOS/assets, because
# the binary resolves assets next to the executable (see
# frontends/gpui/src/render/builders/icon.rs) while codesign requires
# non-code files to live outside Contents/MacOS.

if [ $# -ne 2 ]; then
    echo "Usage: $0 <path-to-holon-gpui-binary> <output-dir>" >&2
    exit 1
fi

BIN="$1"
OUT="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$OUT/Holon.app"

for f in "$BIN" "$ROOT/frontends/gpui/macos/Info.plist" "$ROOT/assets/images/holon.icns"; do
    if [ ! -e "$f" ]; then
        echo "ERROR: required file missing: $f" >&2
        exit 1
    fi
done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/frontends/gpui/macos/Info.plist" "$APP/Contents/Info.plist"
cp "$BIN" "$APP/Contents/MacOS/holon-gpui"
chmod +x "$APP/Contents/MacOS/holon-gpui"
cp "$ROOT/assets/images/holon.icns" "$APP/Contents/Resources/holon.icns"
# Copy assets dereferencing symlinks and dropping dangling ones. A plain
# cp -R would copy the dead assets/queries/*.prql links into the bundle
# (and codesign then chokes on out-of-tree symlinks). See the script.
bash "$ROOT/scripts/stage-assets.sh" "$ROOT/assets" "$APP/Contents/Resources/assets"
# This second symlink is intentional and internal to the bundle (the binary
# resolves assets next to the executable); it resolves within Contents/, so
# it is fine to keep as a link.
ln -s ../Resources/assets "$APP/Contents/MacOS/assets"

echo "Bundle ready: $APP"
