#!/usr/bin/env bash
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/sql-loss
cargo nextest run -p holon-integration-tests \
  --features holon-integration-tests/pbt \
  --test two_instance_composed_pbt --test-threads 4
