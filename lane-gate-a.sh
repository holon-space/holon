#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/agent-adacd27e03318e513 || exit 97
test -f crates/holon-pbt-core/src/step_vocabulary.rs || exit 98
echo "== fmt =="
cargo fmt --all -- --check
echo "fmt-exit=$?"
echo "== workspace check =="
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  cargo check --workspace --all-targets \
  --features holon-integration-tests/pbt,holon-gpui/pbt
echo "check-exit=$?"
