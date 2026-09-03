#!/usr/bin/env bash
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/sql-loss
echo "=== fmt"; cargo fmt --all --check
echo "=== staged_parents unit tests"
cargo nextest run -p holon --lib --test-threads 4 staged_parents_tests
