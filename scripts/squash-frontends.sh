#!/usr/bin/env bash
set -euo pipefail

# Squash directory-specific changes from the working copy (@) into jj
# bookmarks whose name matches the directory path. Discovers candidates
# by listing all bookmarks and checking if a corresponding directory exists.

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
    echo "=== DRY RUN ==="
fi

jj bookmark list --template 'name ++ "\n"' 2>/dev/null | while read -r bookmark; do
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
