#!/usr/bin/env bash
# The second half of the landing gate: `just timing-gate` under its quiet hold,
# on exactly the tip scripts/gate-integrate.sh built, in that checkout (the
# current directory).
#
# Usage, inside a build slot (`flow job run ... -- scripts/gate-timing.sh ...`):
#   scripts/gate-timing.sh <gate-id> "<cards>" <tip>
#
# Exit: 9 = the checkout is not <tip>, checked before any cargo command;
# 10 = the timing binaries were not built yet, so the hold would time a compile;
# otherwise `just timing-gate`'s exit (3 = no verdict on a busy host).
set -euo pipefail

[ "$#" -eq 3 ] || { echo "usage: $0 <gate-id> \"<cards>\" <tip>" >&2; exit 2; }
export GATE=$1 GATE_CARDS=$2 HOLON_GATE_ID=$1
tip=$3
# The gpui build script then ignores git, so a git or jj call between the two
# scripts cannot rebuild what gate-integrate.sh built.
export HOLON_BUILD_SHA=${tip:0:12}

[ "$(jj log -r @- --no-graph -T 'commit_id')" = "$tip" ] || { echo "WRONG TREE: @- is not the integrated tip $tip"; exit 9; }
[ -z "$(jj diff -r @ --summary)" ] || { echo "WRONG TREE: @ carries edits on top of the integrated tip $tip"; exit 9; }

mkdir -p target/gate-logs
echo "gate-timing.sh: $GATE (pid $$), tip $tip, cards: $GATE_CARDS"
# shellcheck source=/dev/null
source ~/.claude/skills/orchestrator/scripts/gate-step.sh

build_log=target/gate-logs/$GATE-timing-build.log
step 17 timing-gate-build just timing-gate-build 2>&1 | tee "$build_log"
! grep -q '^ *Compiling ' "$build_log" || { echo "GATE COMPILED: the timing binaries of $tip were not built by gate-integrate.sh (see $build_log)"; exit 10; }
rc=0
step 16 timing-gate just timing-gate || rc=$?
/usr/bin/python3 "$(dirname "$0")/target-gc.py" target --apply --incremental-below "${GATE_DISK_FLOOR_GIB:-130}" || echo "TARGET-GC NOT RUN (exit $?), see above"
exit $rc
