#!/usr/bin/env bash
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/sql-loss
echo "=== fmt"; cargo fmt --all --check
echo "=== units"
cargo nextest run -p holon-loro -p holon-core -p holon-turso -p holon-app -p holon-architecture-tests --test-threads 4
echo "=== loro-suite"; just loro-suite
echo "=== gpui check"; cargo check -p holon-gpui
