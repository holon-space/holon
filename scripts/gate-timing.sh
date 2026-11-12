#!/usr/bin/env bash
# The second half of the landing gate: `just timing-gate` under its quiet hold,
# on exactly the tip scripts/gate-integrate.sh built, in that checkout (the
# current directory).
#
# Usage, inside a build slot (`flow job run ... -- scripts/gate-timing.sh ...`):
#   scripts/gate-timing.sh <gate-id> "<cards>" <tip>
#   scripts/gate-timing.sh --steps   prints the step names, for the selection
# A run starts itself under gate-step.sh's runner (GATE_FULL=1 unless GATE_SELECT=1),
# which writes its gate-ends row.
#
# Exit: 9 = the checkout is not <tip>, checked before any cargo command;
# a red step 17 ends the gate with its exit (1 = scripts/check-compile-shape.py
#   found the gate's cargo calls in more than one compile shape, or no record of them);
# 10 = the timing binaries were not built yet, so the hold would time a compile;
# a red `just timing-gate` that the runner's record names: the script exits 0 and the
#   runner 1 (the step's own exit, 3 = no verdict on a busy host, is in its row and log);
# any other non-zero step exit (78: no build slot) ends the gate with that exit.
# At its end it prints the gate's block of scripts/timing-breaches.py.
set -euo pipefail

STEPS="timing-gate-build timing-gate"
if [ "${1:-}" = --steps ]; then echo $STEPS; exit; fi
if [ -z "${GATE_RECORD:-}" ]; then
  [ "${GATE_SELECT:-}" = 1 ] || export GATE_FULL=1
  exec bash ~/.claude/skills/orchestrator/scripts/gate-step.sh run "$0" "$@"
fi
[ "$#" -eq 3 ] || { echo "usage: $0 <gate-id> \"<cards>\" <tip>" >&2; exit 2; }
export GATE=$1 GATE_CARDS=$2 HOLON_GATE_ID=$1 GATE_TIP=$3
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
runner_mode() { [ "${GATE_FULL:-}" = 1 ] || [ "${GATE_SELECT:-}" = 1 ]; }
# A step sees neither the record file of gate-step.sh's runner nor its temp folder.
# Returns the step's exit, which `step` under the runner only writes to the record.
gate_step() {
  step "$1" "$2" env -u GATE_RECORD -u GATE_SELECTED -u GATE_CHANGED TMPDIR="$GATE_STEP_TMP" "${@:3}" || return
  [ -z "${GATE_RECORD:-}" ] || return "$(awk -v l="red [$1] $2 (exit " 'index($0, l) == 1 { rc = substr($0, length(l) + 1) + 0 } END { print rc + 0 }' "$GATE_RECORD")"
}
# Prints the lines of step $1 $2 that ended with $3; fails for a red the runner's record
# does not name, as the step then did not run.
report() {
  echo "step [$1] $2: exit $3"
  [ "$3" != 0 ] || return 0
  if runner_mode && ! grep -qxF "red [$1] $2 (exit $3)" "$GATE_RECORD"; then
    echo "RED STEP [$1] $2: exit $3 without running"
    return 1
  fi
  echo "RED STEP [$1] $2: exit $3"
}

build_log=target/gate-logs/$GATE-timing-build.log
rc=0
gate_step 17 timing-gate-build bash -c '"$0" "$1" && just timing-gate-build' \
  "$(dirname "$0")/check-compile-shape.py" "$HOLON_CARGO_RECORD" 2>&1 | tee "$build_log" || rc=$?
report 17 timing-gate-build "$rc" || :
[ "$rc" = 0 ] || exit "$rc"
! grep -q '^ *Compiling ' "$build_log" || { echo "GATE COMPILED: the timing binaries of $tip were not built by gate-integrate.sh (see $build_log)"; exit 10; }
rc=0
gate_step 16 timing-gate just timing-gate || rc=$?
if [ "$rc" = 0 ]; then
  report 16 timing-gate 0
  /usr/bin/python3 "$(dirname "$0")/target-gc.py" target --apply --floor "$(just --justfile "$(dirname "$0")/../justfile" --evaluate GATE_DISK_FLOOR_GIB)" || echo "TARGET-GC NOT RUN (exit $?), see above"
  exit 0
fi
# A red run keeps its build dirs for the rerun.
echo "target-gc skipped: the timing run is red (exit $rc)"
report 16 timing-gate "$rc" || exit "$rc"
# The runner ends the gate red from its record.
runner_mode || exit "$rc"
exit 0
