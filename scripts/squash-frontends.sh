#!/usr/bin/env bash
set -euo pipefail

# Squash directory-specific changes from the working copy (@) into jj
# bookmarks whose name matches the directory path.
#
# Usage: squash-frontends.sh [--dry-run] [--from REV] [folder ...]
#   No folders: discovers candidates from all bookmarks with matching directories.
#   With folders: only processes the specified folders.

DRY_RUN=false
FROM=""
FOLDERS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --from)
            FROM="$2"
            shift 2
            ;;
        *)
            FOLDERS+=("$1")
            shift
            ;;
    esac
done

if [[ "$DRY_RUN" == true ]]; then
    echo "=== DRY RUN ==="
fi

if [[ ${#FOLDERS[@]} -gt 0 ]]; then
    bookmarks=("${FOLDERS[@]}")
else
    bookmarks=()
    while IFS= read -r line; do
        [[ -n "$line" ]] && bookmarks+=("$line")
    done < <(jj bookmark list --template 'name ++ "\n"' 2>/dev/null)
fi

for bookmark in "${bookmarks[@]}"; do
    [[ -d "$bookmark" ]] || continue

    dir="$bookmark/"
    changes=$(jj diff --stat -- "$dir" 2>/dev/null | tail -1)
    if [[ -z "$changes" ]] || echo "$changes" | grep -qE "^0 files changed"; then
        echo "SKIP $bookmark: no changes in $dir"
        continue
    fi

    echo "SQUASH $dir → $bookmark"
    echo "  $changes"

    if [[ "$DRY_RUN" == false ]]; then
        squash_args=(squash --into "$bookmark")
        if [[ -n "$FROM" ]]; then
            squash_args+=(--from "$FROM")
            echo "  from $FROM"
        fi
        squash_args+=(-- "$dir")
        jj "${squash_args[@]}"
    fi
done
