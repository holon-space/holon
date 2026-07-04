#!/usr/bin/env bash
# Copy an assets tree for packaging, cross-platform (Linux / macOS / Git-Bash
# on Windows). Real files are copied as-is; symlinks are DEREFERENCED to their
# target contents; dangling symlinks are dropped with a loud warning.
#
# Why not `cp -R`/`cp -RL`:
#   * `cp -R` recreates symlinks — assets/queries/*.prql point OUT of the tree
#     (into crates/*/queries via ../../../..), so the copies dangle, and on
#     Windows `cp` fails outright trying to create them.
#   * `cp -RL` dereferences, but STATS every link first — it aborts on the
#     dangling ones (two of the three query links have no target: the
#     holon-todoist crate was removed; crates/holon/queries/undo_state.prql
#     never existed). See git history / docs/Operations/ReleasePipeline.md.
# Dropping a dangling link changes nothing at runtime: it resolves to nothing
# on the user's machine anyway. We warn so the drop is never silent.
set -euo pipefail

src="${1:?usage: stage-assets.sh SRC DEST}"
dest="${2:?usage: stage-assets.sh SRC DEST}"
mkdir -p "$dest"

# Directories first (so file copies below have somewhere to land).
while IFS= read -r d; do
  mkdir -p "$dest/$d"
done < <(cd "$src" && find . -type d)

# Real files (find -type f does not match symlinks).
while IFS= read -r f; do
  cp "$src/$f" "$dest/$f"
done < <(cd "$src" && find . -type f)

# Symlinks: dereference if the target resolves, else warn + skip.
while IFS= read -r l; do
  if [ -e "$src/$l" ]; then
    cp -L "$src/$l" "$dest/$l"
  else
    echo "::warning::stage-assets: dropping dangling symlink $l -> $(readlink "$src/$l")"
  fi
done < <(cd "$src" && find . -type l)
