#!/usr/bin/env bash
# The second half of the landing gate: `just timing-gate` under its quiet hold,
# on exactly the tip scripts/gate-integrate.sh built, in that checkout (the
# current directory).
#
# Usage, inside a build slot (`flow job run ... -- scripts/gate-timing.sh ...`):
#   scripts/gate-timing.sh <gate-id> "<cards>" <tip>
#   scripts/gate-timing.sh --steps   prints the step names, for the selection
#
# Exit: 9 = the checkout is not <tip>, checked before any cargo command;
# 1 from step 17 = scripts/check-compile-shape.py found the gate's cargo calls
#   in more than one compile shape, or no record of them;
# 10 = the timing binaries were not built yet, so the hold would time a compile;
# otherwise `just timing-gate`'s exit (3 = no verdict on a busy host).
# At its end it prints the gate's block of scripts/timing-breaches.py.
set -euo pipefail

STEPS="timing-gate-build timing-gate"
if [ "${1:-}" = --steps ]; then echo $STEPS; exit; fi
[ "$#" -eq 3 ] || { echo "usage: $0 <gate-id> \"<cards>\" <tip>" >&2; exit 2; }
export GATE=$1 GATE_CARDS=$2 HOLON_GATE_ID=$1
tip=$3

[ "$(jj log -r @- --no-graph -T 'commit_id')" = "$tip" ] || { echo "WRONG TREE: @- is not the integrated tip $tip"; exit 9; }
[ -z "$(jj diff -r @ --summary)" ] || { echo "WRONG TREE: @ carries edits on top of the integrated tip $tip"; exit 9; }

mkdir -p target/gate-logs
export HOLON_CARGO_RECORD=$PWD/target/gate-logs/$GATE-cargo-calls.jsonl
# shellcheck source=scripts/cargo-shape-env.sh
source "$(dirname "$0")/cargo-shape-env.sh" gate
echo "gate-timing.sh: $GATE (pid $$), tip $tip, cards: $GATE_CARDS"
shape_env_header
# shellcheck source=/dev/null
source ~/.claude/skills/orchestrator/scripts/gate-step.sh
# shellcheck disable=SC2086
gate_steps $STEPS
GATE_STEP_TMP=$(mktemp -d)
# The breach summary only reports: the EXIT trap keeps the gate's exit code.
trap 'rm -rf "$GATE_STEP_TMP"; /usr/bin/python3 "$(dirname "$0")/timing-breaches.py" "$GATE" || echo "TIMING BREACHES NOT SUMMARIZED (exit $?)"' EXIT
# A step sees neither the record file of gate-step.sh's runner nor its temp folder.
gate_step() {
  step "$1" "$2" env -u GATE_RECORD -u GATE_SELECTED -u GATE_CHANGED TMPDIR="$GATE_STEP_TMP" "${@:3}"
}

build_log=target/gate-logs/$GATE-timing-build.log
gate_step 17 timing-gate-build bash -c '"$0" "$1" && just timing-gate-build' \
  "$(dirname "$0")/check-compile-shape.py" "$HOLON_CARGO_RECORD" 2>&1 | tee "$build_log"
! grep -q '^ *Compiling ' "$build_log" || { echo "GATE COMPILED: the timing binaries of $tip were not built by gate-integrate.sh (see $build_log)"; exit 10; }
rc=0
gate_step 16 timing-gate just timing-gate || rc=$?
# A red run keeps its build dirs for the rerun.
if [ "$rc" -ne 0 ]; then
  echo "target-gc skipped: the timing run is red (exit $rc)"
else
  /usr/bin/python3 "$(dirname "$0")/target-gc.py" target --apply --floor "$(just --justfile "$(dirname "$0")/../justfile" --evaluate GATE_DISK_FLOOR_GIB)" || echo "TARGET-GC NOT RUN (exit $?), see above"
fi
exit $rc
