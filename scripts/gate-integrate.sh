#!/usr/bin/env bash
# The first half of the landing gate: every gate step that judges no duration,
# in the integration checkout that gate-combine.sh prepared (the
# current directory). It also builds every binary `just timing-gate` runs, so
# scripts/gate-timing.sh compiles nothing.
#
# Usage, inside a build slot (`flow job run ... -- scripts/gate-integrate.sh ...`):
#   scripts/gate-integrate.sh <gate-id> "<cards>" <tip>
#   GATE_DISK_FLOOR_GIB  free disk the gate needs (default 130).
#
#   scripts/gate-integrate.sh --steps   prints the step names, for the selection
#
# After gate-compile the steps run at once in TRACKS, one build slot per track:
# no step reads another step's output. The steps of QUIET_TRACK go red on a
# loaded host, so that track runs under the machine-wide quiet hold when one of
# them is selected; the other tracks run at any load. A track with no selected
# step runs in the gate's own slot. Each step writes its own log and
# gate-steps.jsonl row; the logs follow in TRACKS order.
#
# Last stdout line on green: `candidate tip: <tip>`, the argument for
# scripts/gate-timing.sh.
# Exit 0: every step green. A red step before gate-compile stops the run with
# its exit code; 9 = the checkout is not <tip>, 8 = free disk below the floor.
# After gate-compile every step runs to its end, each red step is named, and
# the run exits with the code of the first red one in TRACKS order; a red extra
# nextest alone exits 12 or 13.
set -euo pipefail

# Balanced by step time; the first track runs in the
# gate's own slot. A step is <n>:<name>, its number in gate-steps.jsonl.
TRACKS=(
  "13:hand-authored 17:timing-gate-build 7:gate-arch 14:projector-lag-lock 5:check-dioxus-web-wasm 6:check-worker-wasm 8:arch-validate 9:featuremap 18:guests-verify"
  "19:extra-nextest 15:windowed-gate"
  "11:keystone-smoke 3:check-release-shape 4:check-frontend-wasm 12:loro-suite 10:analyze-arch 22:keystone-tour"
)
QUIET_TRACK=1
read -ra STEPS <<<"${TRACKS[*]}"
step_names() { echo fmt deny gate-compile "${STEPS[@]#*:}"; }
if [ "${1:-}" = --steps ]; then step_names; exit; fi
step_file() { echo "target/gate-logs/$GATE-step-$1.$2"; }
SLOT=~/.claude/skills/orchestrator/scripts/with-build-slot.sh
# The hold names the gate's own pid, and every track names it as its holder
# (HOLON_BUILD_QUIET_HOLDER), so no track of this gate is lowered. Exit
# 75 is a hold of another job (a timing gate or probe): wait for its release.
quiet_hold() {
  local rc
  while :; do
    rc=0; "$SLOT" --quiet-hold "$GATE_PID" || rc=$?
    [ "$rc" = 75 ] || break
    sleep "${GATE_STEP_POLL_S:-5}"
  done
  [ "$rc" = 0 ] || { echo "QUIET HOLD NOT TAKEN: with-build-slot.sh --quiet-hold exit $rc"; exit "$rc"; }
  quiet_age_watch & watch=$!
  trap '"$SLOT" --quiet-release "$GATE_PID"; ! kill -0 "$watch" 2>/dev/null || kill "$watch"; [ -z "${TRACK_TMP:-}" ] || rm -rf "$TRACK_TMP"' EXIT
}
# with-build-slot.sh treats a hold older than its age limit (3 h) as free, so
# another job may then take it while this track still runs.
quiet_age_watch() {
  local s
  trap 'kill "$s"; exit' TERM
  sleep "${GATE_QUIET_MAX_AGE_S:-10800}" >/dev/null & s=$!
  wait "$s"
  echo "quiet hold may have expired; reds after $(date -u +%Y-%m-%dT%H:%M:%SZ) may be load reds"
}
# Whether the batch's selection runs step $1 (every step runs without one).
selected() {
  [ "${GATE_SELECT:-}" = 1 ] && [ "${GATE_FULL:-}" != 1 ] || return 0
  awk -v s="$1" '$1 == s || $1 == "*" { f = 1 } END { exit !f }' "$GATE_SELECTED"
}
track_selected() {
  local entry
  for entry in ${TRACKS[$1]}; do selected "${entry#*:}" && return; done
  return 1
}
# A step sees neither the record file of gate-step.sh's runner nor its temp folder.
gate_step() {
  step "$1" "$2" env -u GATE_RECORD -u GATE_SELECTED -u GATE_CHANGED TMPDIR="$GATE_STEP_TMP" "${@:3}"
}
track() {
  local entry n name rc
  [ -n "${GATE_STEP_TMP:-}" ] || { GATE_STEP_TMP=$(mktemp -d); TRACK_TMP=$GATE_STEP_TMP; trap 'rm -rf "$TRACK_TMP"' EXIT; }
  [ "$1" != "$QUIET_TRACK" ] || ! track_selected "$1" || quiet_hold
  for entry in ${TRACKS[$1]}; do
    n=${entry%%:*} name=${entry#*:}
    case $name in
      featuremap) set -- just cargo-free-checks ;;
      keystone-smoke | hand-authored | extra-nextest) set -- bash "$(dirname "$0")/gate-integrate-steps.sh" "$name" ;;
      loro-suite | projector-lag-lock) set -- env HOLON_PBT_TIMING=measure just "$name" ;;
      *) set -- just "$name" ;;
    esac
    rc=0; gate_step "$n" "$name" "$@" >"$(step_file "$n" log)" 2>&1 || rc=$?
    echo "$rc" >"$(step_file "$n" rc)"
    echo "step [$n] $name: exit $rc"
  done
}

# shellcheck source=/dev/null
source ~/.claude/skills/orchestrator/scripts/gate-step.sh
# shellcheck disable=SC2046
gate_steps $(step_names)
[ "${1:-}" != --track ] || { track "$2"; exit; }

[ "$#" -eq 3 ] || { echo "usage: $0 <gate-id> \"<cards>\" <tip>" >&2; exit 2; }
export GATE=$1 GATE_CARDS=$2 HOLON_GATE_ID=$1 GATE_PID=$$ HOLON_BUILD_QUIET_HOLDER=$$
tip=$3
DISK_FLOOR_GIB=${GATE_DISK_FLOOR_GIB:-130}

[ "$(jj log -r @- --no-graph -T 'commit_id')" = "$tip" ] || { echo "WRONG TREE: @- is not the tip $tip"; exit 9; }
[ -z "$(jj diff -r @ --summary)" ] || { echo "WRONG TREE: @ carries edits on top of the tip $tip"; exit 9; }
[ "$(df -g . | awk 'NR==2{print $4}')" -ge "$DISK_FLOOR_GIB" ] || { echo "DISK BELOW $DISK_FLOOR_GIB GiB"; exit 8; }

mkdir -p target/gate-logs
# The tracks inherit the shape env, and scripts/gate-timing.sh checks this
# record of every cargo call before it builds.
export HOLON_CARGO_RECORD=$PWD/target/gate-logs/$GATE-cargo-calls.jsonl
: >"$HOLON_CARGO_RECORD"
# shellcheck source=scripts/cargo-shape-env.sh
source "$(dirname "$0")/cargo-shape-env.sh" gate
PBT_FEATURES=$(just --justfile "$(dirname "$0")/../justfile" --evaluate CANON_FEATURES)
export PBT_FEATURES
echo "gate-integrate.sh: $GATE (pid $$), tip $tip, cards: $GATE_CARDS"
shape_env_header

/usr/bin/python3 "$(dirname "$0")/target-gc.py" target --mark
GATE_STEP_TMP=$(mktemp -d); export GATE_STEP_TMP
trap 'rm -rf "$GATE_STEP_TMP"' EXIT
gate_step 1 fmt cargo fmt --all -- --check
gate_step 20 deny just deny
gate_step 2 gate-compile just gate-compile
rm -f target/gate-logs/"$GATE"-step-*
pids=()
"$0" --track 0 & pids+=($!)
for t in $(seq 1 $((${#TRACKS[@]} - 1))); do
  track_selected "$t" || { "$0" --track "$t" & pids+=($!); continue; }
  # A track of its own takes a slot of its own and writes its own job record.
  env -u HOLON_BUILD_SLOT_HOLDER -u FLOW_JOB_STATUS -u FLOW_JOB_LOG FLOW_JOB_ID="${FLOW_JOB_ID:-$GATE}.track$t" \
    ~/.claude/skills/orchestrator/scripts/with-build-slot.sh "$0" --track "$t" & pids+=($!)
done
red="" extra_red=""
for t in "${!pids[@]}"; do
  rc=0; wait "${pids[$t]}" || rc=$?
  [ "$rc" = 0 ] || { echo "RED TRACK $t: exit $rc"; red=${red:-$rc}; }
done

for entry in "${STEPS[@]}"; do
  n=${entry%%:*} name=${entry#*:}
  [ ! -f "$(step_file "$n" log)" ] || cat "$(step_file "$n" log)"
  rc=$(cat "$(step_file "$n" rc)" 2>/dev/null) || { echo "RED STEP [$n] $name: its track ended before it"; red=${red:-1}; continue; }
  [ "$rc" = 0 ] && continue
  echo "RED STEP [$n] $name: exit $rc, log $(step_file "$n" log)"
  if [ "$name" = extra-nextest ]; then extra_red=$rc; echo "EXTRA NEXTEST NOVEL RED"
  else red=${red:-$rc}; fi
done
[ -z "$red" ] || exit "$red"
[ -z "$extra_red" ] || exit "$extra_red"
echo "INTEGRATE GREEN"
echo "candidate tip: $tip"
