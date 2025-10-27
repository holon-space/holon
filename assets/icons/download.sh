#!/usr/bin/env bash
set -euo pipefail

# Downloads Fluent Emoji (Flat style) SVGs listed in manifest.toml
# Usage:
#   ./download.sh        # download missing icons
#   ./download.sh sync   # download missing + remove unlisted icons
#   ./download.sh list   # show what would be downloaded

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MANIFEST="$SCRIPT_DIR/manifest.toml"
BASE_URL="https://raw.githubusercontent.com/microsoft/fluentui-emoji/main/assets"

if [[ ! -f "$MANIFEST" ]]; then
  echo "Error: manifest.toml not found at $MANIFEST" >&2
  exit 1
fi

# Parse manifest: extract lines like 'key = "Emoji Name"' under [icons]
parse_manifest() {
  local in_icons=false
  while IFS= read -r line; do
    line="${line%%#*}"           # strip comments
    line="${line#"${line%%[![:space:]]*}"}"  # trim leading whitespace
    [[ -z "$line" ]] && continue

    if [[ "$line" == "[icons]" ]]; then
      in_icons=true
      continue
    elif [[ "$line" == "["* ]]; then
      in_icons=false
      continue
    fi

    if $in_icons; then
      local key value
      key="$(echo "$line" | sed 's/ *=.*//')"
      value="$(echo "$line" | sed 's/[^"]*"//; s/".*//')"
      [[ -n "$key" && -n "$value" ]] && echo "$key|$value"
    fi
  done < "$MANIFEST"
}

# Convert "Emoji Name" → "emoji_name" for the filename in the repo
to_repo_filename() {
  echo "$1" | tr '[:upper:]' '[:lower:]' | tr ' ' '_'
}

# URL-encode spaces in emoji name for the folder path
url_encode_name() {
  echo "$1" | sed 's/ /%20/g'
}

entries="$(parse_manifest)"
cmd="${1:-download}"

case "$cmd" in
  list)
    echo "Icons in manifest:"
    while IFS='|' read -r key name; do
      local_file="$SCRIPT_DIR/${key}.svg"
      if [[ -f "$local_file" ]]; then
        echo "  ✓ ${key}.svg  ← \"$name\""
      else
        echo "  ✗ ${key}.svg  ← \"$name\"  (missing)"
      fi
    done <<< "$entries"
    ;;

  download|sync)
    downloaded=0
    skipped=0
    failed=0

    while IFS='|' read -r key name; do
      local_file="$SCRIPT_DIR/${key}.svg"

      if [[ -f "$local_file" ]]; then
        skipped=$((skipped + 1))
        continue
      fi

      repo_name="$(to_repo_filename "$name")"
      encoded_folder="$(url_encode_name "$name")"
      url="${BASE_URL}/${encoded_folder}/Flat/${repo_name}_flat.svg"

      if curl -fsSL -o "$local_file" "$url" 2>/dev/null; then
        echo "  ↓ ${key}.svg"
        downloaded=$((downloaded + 1))
      else
        echo "  ✗ ${key}.svg  (failed: $url)" >&2
        failed=$((failed + 1))
      fi
    done <<< "$entries"

    echo ""
    echo "Done: $downloaded downloaded, $skipped already present, $failed failed"

    if [[ "$cmd" == "sync" ]]; then
      removed=0
      # Collect expected filenames
      expected=""
      while IFS='|' read -r key _; do
        expected="$expected ${key}.svg"
      done <<< "$entries"

      for svg in "$SCRIPT_DIR"/*.svg; do
        [[ -f "$svg" ]] || continue
        basename="$(basename "$svg")"
        if [[ " $expected " != *" $basename "* ]]; then
          rm "$svg"
          echo "  🗑 $basename (not in manifest)"
          removed=$((removed + 1))
        fi
      done
      [[ $removed -gt 0 ]] && echo "Removed $removed unlisted icon(s)"
    fi
    ;;

  *)
    echo "Usage: $0 [download|sync|list]" >&2
    exit 1
    ;;
esac
