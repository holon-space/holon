#!/usr/bin/env bash
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/sql-loss
echo "=== batch_delete_cascade_updates"
cargo nextest run -p holon --test batch_delete_cascade_updates --test-threads 4 || echo "T1_EXIT=$?"
echo "=== staged_parents_tests"
cargo nextest run -p holon --lib --test-threads 4 staged_parents_tests || echo "T2_EXIT=$?"
echo "=== pin x3 (shipped code)"
for i in 1 2 3; do
  echo "=== pin run $i"
  cargo nextest run -p holon-integration-tests --features holon-integration-tests/pbt \
    --test two_instance_composed_pbt --test-threads 1 --no-fail-fast \
    owner_heavy_indent_then_join_stalls_the_receiver_projection || echo "PIN_EXIT_$i=$?"
done
