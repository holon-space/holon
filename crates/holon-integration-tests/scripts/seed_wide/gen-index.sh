#!/usr/bin/env bash
# Materialize `index.org` (gitignored) from its two checked-in sources: the
# pinned layout-doc `#+ID:` line and the app's default layout asset.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HEADER="$SCRIPT_DIR/index.org.header"
ASSET="$SCRIPT_DIR/../../../../assets/default/index.org"
OUT="$SCRIPT_DIR/index.org"

for src in "$HEADER" "$ASSET"; do
  [ -f "$src" ] || { echo "ERROR: missing $src — cannot generate $OUT" >&2; exit 1; }
done

tmp="$(mktemp "$SCRIPT_DIR/.index.org.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
cat "$HEADER" "$ASSET" > "$tmp"
mv "$tmp" "$OUT"
echo "[seed-gen] wrote $OUT"
