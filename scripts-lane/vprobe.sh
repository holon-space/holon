#!/usr/bin/env bash
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/sql-loss
cargo nextest run -p holon --test zz_verify_scratch --test-threads 1 --no-capture --no-fail-fast || echo "EXIT=$?"
