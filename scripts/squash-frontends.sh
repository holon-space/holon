#!/usr/bin/env bash
set -euo pipefail

# Squash directory-specific changes from the working copy (@) into jj
# bookmarks whose name matches the directory path.
#
# Usage: squash-frontends.sh [--dry-run] [folder ...]
#   No folders: discovers candidates from all bookmarks with matching directories.
#   With folders: only processes the specified folders.

DRY_RUN=false
FOLDERS=()
for arg in "$@"; do
    if [[ "$arg" == "--dry-run" ]]; then
        DRY_RUN=true
    else
        FOLDERS+=("$arg")
    fi
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
    if [[ -z "$changes" ]] || echo "$changes" | grep -q "0 files changed"; then
        echo "SKIP $bookmark: no changes in $dir"
        continue
    fi

    echo "SQUASH $dir → $bookmark"
    echo "  $changes"

    if [[ "$DRY_RUN" == false ]]; then
        jj squash --into "$bookmark" -- "$dir"
    fi
done
