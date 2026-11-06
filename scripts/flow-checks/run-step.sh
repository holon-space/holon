#!/usr/bin/env bash
# Run one step line that scripts/select-tests.py printed and write its log to stdout:
#   BASE=<lane base commit> scripts/flow-checks/run-step.sh '<step>: <args>' | tee target/<lane>/<step>.log
# Line 1 is the step line verbatim, then the BASE:/TREE:/STARTED: header and the step's output; the last line is
# `exit: <code>`, and the script exits with that code. scripts/flow-checks/tests-cover-diff.py reads this shape.
set -uo pipefail
line=$1
: "${BASE:?BASE=<lane base commit> must be set}"
step=${line%%: *}
args=${line#*: }
echo "$line"
echo "BASE: $BASE"
echo "TREE: $(jj log -r @ --no-graph -T commit_id)"
echo "STARTED: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
# shellcheck source=scripts/cargo-shape-env.sh
source "$(dirname "$0")/../cargo-shape-env.sh" lane || { echo; echo "exit: 1"; exit 1; }
shape_env_header
# The args hold shell quoting (`-E '<filterset>'`).
eval "set -- $args"
case $step in
  clippy) cargo clippy "$@" --all-targets -- -D warnings 2>&1 ;;
  nextest) HOLON_PBT_TIMING=${HOLON_PBT_TIMING-measure} "$(dirname "$0")/../lane-nextest.py" run "$@" 2>&1 ;;
  *) "$@" 2>&1 ;;
esac
code=$?
echo
echo "exit: $code"
exit "$code"
