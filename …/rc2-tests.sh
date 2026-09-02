#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/rc2-inert || exit 97
test -f crates/holon-core/src/integration_attribution.rs || exit 98
echo "tree: $(pwd)"
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  bash lane-scripts/rc2-tests-inner.sh
echo "exit=$?"
