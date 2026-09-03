#!/usr/bin/env bash
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/sql-loss
export RUST_LOG='warn,holon_loro::loro_sync_controller=trace'
cargo nextest run -p holon-integration-tests \
  --features holon-integration-tests/pbt \
  --test two_instance_composed_pbt --run-ignored all \
  --test-threads 1 --no-capture \
  owner_heavy_indent_then_join_stalls_the_receiver_projection
