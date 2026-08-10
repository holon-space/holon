#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/agent-adacd27e03318e513 || exit 97
test -f lane-report-77inc2.md || exit 98
echo "== fmt =="
cargo fmt --all -- --check
echo "fmt-exit=$?"
echo "== macros + vocabulary laws + headless feature replay =="
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  cargo test -p holon-macros --lib step_vocabulary
echo "macros-exit=$?"
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  cargo test -p holon-integration-tests --features holon-integration-tests/pbt \
  --test step_vocabulary_laws --test step_vocabulary_agreement --test split_block_content_pbt
echo "headless-exit=$?"
echo "== windowed =="
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay
echo "windowed-exit=$?"
echo "== hand-authored =="
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  just hand-authored
echo "hand-exit=$?"
