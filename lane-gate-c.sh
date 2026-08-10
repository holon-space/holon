#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/agent-adacd27e03318e513 || exit 97
test -f crates/holon-integration-tests/tests/step_vocabulary_laws.rs || exit 98
echo "== windowed gherkin replay =="
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay
echo "windowed-exit=$?"
