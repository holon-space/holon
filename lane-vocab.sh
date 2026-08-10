#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/agent-adacd27e03318e513 || exit 97
test -f crates/holon-integration-tests/tests/step_vocabulary_laws.rs || exit 98
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  cargo test -p holon-integration-tests --features holon-integration-tests/pbt \
  --test step_vocabulary_laws --test step_vocabulary_agreement
echo "cargo-exit=$?"
