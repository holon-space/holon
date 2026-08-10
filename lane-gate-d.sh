#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/agent-adacd27e03318e513 || exit 97
test -f crates/holon-integration-tests/tests/step_vocabulary_laws.rs || exit 98
echo "== keystone-smoke =="
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  just keystone-smoke
echo "smoke-exit=$?"
echo "== hand-authored =="
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  just hand-authored
echo "hand-exit=$?"
