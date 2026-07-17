#!/usr/bin/env bash
# Export the Holon app icon into every publishing format and wire the symlinks
# that Fastlane / Xcode / the macOS bundler expect.
#
#   scripts/export-app-icon.sh
#
# Layout is TRACED from curated node data (positions/sizes/opacity extracted from
# the reference icon; see scripts/detect_nodes.py). Two densities are rendered:
#   • FULL  (18 nodes) for large formats
#   • SMALL (6 nodes)  for tiny sizes (≤ SMALL_MAX px), where clutter hurts
# The generator is a self-contained uv script (dep: igraph).
#
# Requires: uv, rsvg-convert, iconutil (macOS), magick (ImageMagick).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GEN="$ROOT/scripts/gen_holon_icon.py"
OUT="$ROOT/assets/icons/app"          # namespaced: the sibling glyph library owns assets/icons/*.svg

NODES_FULL="$OUT/holon-icon.nodes.json"        # 18-node source of truth
NODES_SMALL="$OUT/holon-icon-small.nodes.json" # 6-node source of truth
FILL_FULL=0.88
FILL_SMALL=1.20
SMALL_MAX=64          # raster sizes ≤ this are rendered from the 6-node data

echo "→ regenerating $OUT (preserving the *.nodes.json sources)"
rm -rf "$OUT"/web "$OUT"/png "$OUT"/ios "$OUT"/android "$OUT"/macos "$OUT"/*.svg "$OUT"/holon.icns
mkdir -p "$OUT/web" "$OUT/png" "$OUT/ios" "$OUT/android" "$OUT/macos"
# assets/icons/.gitignore ignores *.svg (glyph library is downloaded); re-include ours.
printf '# app-icon SVGs are generated + tracked (parent .gitignore ignores *.svg)\n!*.svg\n' > "$OUT/.gitignore"

gen () {  # gen <json> <fill> <out.svg> [--square|--transparent|--bg-only]
  HOLON_TRACE_SRC="$1" HOLON_FILL_FRACT="$2" uv run --script "$GEN" "$3" ${4:+$4} --trace
}
echo "→ master SVGs (full=18 @${FILL_FULL}, small=6 @${FILL_SMALL})"
gen "$NODES_FULL"  "$FILL_FULL"  "$OUT/holon-icon.svg"
gen "$NODES_FULL"  "$FILL_FULL"  "$OUT/holon-icon.square.svg" --square
gen "$NODES_SMALL" "$FILL_SMALL" "$OUT/holon-icon-small.svg"
gen "$NODES_SMALL" "$FILL_SMALL" "$OUT/holon-icon-small.square.svg" --square

# png <size> <out> [sq]  — picks the 6-node master at/below SMALL_MAX, else 18-node
png () {
  local size="$1" out="$2" sq="${3:-}" base
  if [ "$size" -le "$SMALL_MAX" ]; then base="holon-icon-small"; else base="holon-icon"; fi
  [ "$sq" = sq ] && base="$base.square"
  rsvg-convert -w "$size" -h "$size" "$OUT/$base.svg" -o "$out"
}

echo "→ generic PNGs"
for s in 16 32 48 64 128 256 512 1024; do png "$s" "$OUT/png/holon-$s.png"; done

echo "→ web: favicon (small), apple-touch + PWA (full)"
cp "$OUT/holon-icon-small.svg" "$OUT/web/favicon.svg"
magick "$OUT/png/holon-16.png" "$OUT/png/holon-32.png" "$OUT/png/holon-48.png" "$OUT/web/favicon.ico"
cp "$OUT/png/holon-16.png" "$OUT/web/favicon-16.png"
cp "$OUT/png/holon-32.png" "$OUT/web/favicon-32.png"
png 180 "$OUT/web/apple-touch-icon.png"
png 192 "$OUT/web/icon-192.png" sq
png 512 "$OUT/web/icon-512.png" sq
png 512 "$OUT/web/maskable-512.png" sq

echo "→ iOS app icon (square, no alpha)"
png 1024 "$OUT/ios/AppIcon-1024.png" sq
magick "$OUT/ios/AppIcon-1024.png" -background '#22506b' -alpha remove -alpha off "$OUT/ios/AppIcon-1024.png"

echo "→ Android: Play Store listing icon"
png 512 "$OUT/android/play-icon-512.png" sq
magick "$OUT/android/play-icon-512.png" -background '#22506b' -alpha remove -alpha off "$OUT/android/play-icon-512.png"

echo "→ Android: adaptive launcher icon (res/ tree)"
gen "$NODES_FULL" "$FILL_FULL" "$OUT/android/_fg.svg" --transparent
gen "$NODES_FULL" "$FILL_FULL" "$OUT/android/_bg.svg" --bg-only
RES="$OUT/android/res"
for row in "mdpi 108 48" "hdpi 162 72" "xhdpi 216 96" "xxhdpi 324 144" "xxxhdpi 432 192"; do
  set -- $row; d=$1; full=$2; leg=$3
  safe=$(python3 -c "print(round($full*0.61))")
  mkdir -p "$RES/mipmap-$d"
  rsvg-convert -w "$full" -h "$full" "$OUT/android/_bg.svg" -o "$RES/mipmap-$d/ic_launcher_background.png"
  rsvg-convert -w "$safe" -h "$safe" "$OUT/android/_fg.svg" -o "$OUT/android/_fg_tmp.png"
  magick "$OUT/android/_fg_tmp.png" -background none -gravity center -extent "${full}x${full}" \
         "$RES/mipmap-$d/ic_launcher_foreground.png"
  png "$leg" "$RES/mipmap-$d/ic_launcher.png"                          # legacy fallback (size-split)
done
mkdir -p "$RES/mipmap-anydpi-v26"
for x in ic_launcher ic_launcher_round; do
  cat > "$RES/mipmap-anydpi-v26/$x.xml" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@mipmap/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
XML
done
rm -f "$OUT/android/_fg.svg" "$OUT/android/_bg.svg" "$OUT/android/_fg_tmp.png"

echo "→ macOS .icns (size-split per iconset entry)"
IS="$OUT/macos/holon.iconset"; mkdir -p "$IS"
for s in 16 32 128 256 512; do
  png "$s"        "$IS/icon_${s}x${s}.png"
  png $((s*2))    "$IS/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$IS" -o "$OUT/macos/holon.icns"
rm -rf "$IS"

# ── Symlinks: point every consumer at the generated source of truth ──────────
rel () { python3 -c 'import os,sys; print(os.path.relpath(sys.argv[1], os.path.dirname(sys.argv[2])))' "$1" "$2"; }
link () {
  mkdir -p "$(dirname "$2")"
  ln -sfn "$(rel "$1" "$2")" "$2"
  echo "   $2 → $(readlink "$2")"
}
echo "→ wiring symlinks"
link "$OUT/macos/holon.icns"  "$ROOT/assets/images/holon.icns"
link "$OUT/ios/AppIcon-1024.png" "$ROOT/frontends/gpui/ios/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png"
link "$OUT/android/play-icon-512.png" "$ROOT/frontends/gpui/android/fastlane/metadata/android/en-US/images/icon.png"

echo "✔ done"
find "$OUT" -type f | sort
