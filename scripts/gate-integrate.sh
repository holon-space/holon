#!/usr/bin/env bash
# The first half of the landing gate: every gate step that judges no duration,
# in the integration checkout that gate-combine.sh prepared (the
# current directory). It also builds every binary `just timing-gate` runs, so
# scripts/gate-timing.sh compiles nothing.
#
# Usage, inside a build slot (`flow job run ... -- scripts/gate-integrate.sh ...`):
#   scripts/gate-integrate.sh <gate-id> "<cards>" <tip>
#   GATE_DISK_FLOOR_GIB  free disk the gate needs (default: the justfile's).
#
#   scripts/gate-integrate.sh --steps   prints the step names, for the selection
#   scripts/gate-integrate.sh rerun <gate-id> <tip> <red gate job id>
#     reruns every red step of that gate job on exactly the failed test ids its
#     row names, as a `step` of this job, in the gate's shape env, and appends
#     its cargo calls to the gate's record, so gate-timing.sh after it compiles
#     nothing. Log: target/gate-logs/rerun-<UTC>.log. Exit: 0 when every rerun
#     step is green, else the first red step's; 9 = the checkout is not <tip>;
#     2 = usage, no record of that gate here, no red step, or a red step without
#     failed ids or a rerun of its own (before any test runs).
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
# A full run starts itself under gate-step.sh's runner (GATE_FULL=1 unless
# GATE_SELECT=1), which writes the gate-ends row that flow reads.
# Exit 0: every step green. A red step before gate-compile stops the run with
# its exit code; 9 = the checkout is not <tip>, 8 = free disk below the floor,
# 2 = usage or a <tip> that is no full commit id.
# After gate-compile every step runs to its end and each red step is named. A
# track that ended early, or a step that returned without running, exits with
# its code. Under the runner a run red in
# these steps only exits 0, and the runner ends it with exit 1. Without the
# runner it exits with the code of the first red step in TRACKS order; a red
# extra nextest alone exits 12 or 13, or 2 when its filter set is unparsable.
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
step_names() { echo fmt deny machete gate-compile "${STEPS[@]#*:}"; }
if [ "${1:-}" = --steps ]; then step_names; exit; fi
if [ -z "${GATE_RECORD:-}" ] && [ "${1:-}" != --track ] && [ "${1:-}" != rerun ]; then
  [ "${GATE_SELECT:-}" = 1 ] || export GATE_FULL=1
  exec bash ~/.claude/skills/orchestrator/scripts/gate-step.sh run "$0" "$@"
fi
step_file() { echo "target/gate-logs/$GATE-step-$1.$2"; }
SLOT=~/.claude/skills/orchestrator/scripts/with-build-slot.sh
# The hold names the gate's own pid, and every track names it as its holder
# (HOLON_BUILD_QUIET_HOLDER), so no track of this gate is lowered.
quiet_hold() {
  "$(dirname "$0")/quiet-wait.sh" "$SLOT" "$GATE_PID" || exit
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
# Its row names the gate's job, also in a track that holds a slot under a job id of its own.
# Returns the step's exit, which `step` under the runner only writes to the record.
gate_step() {
  FLOW_JOB_ID=${GATE_JOB_ID:-${FLOW_JOB_ID:-}} step "$1" "$2" env -u GATE_RECORD -u GATE_SELECTED -u GATE_CHANGED TMPDIR="$GATE_STEP_TMP" "${@:3}" || return
  [ -z "${GATE_RECORD:-}" ] || return "$(awk -v l="red [$1] $2 (exit " 'index($0, l) == 1 { rc = substr($0, length(l) + 1) + 0 } END { print rc + 0 }' "$GATE_RECORD")"
}
track() {
  local entry n name rc
  if [ "$1" != 0 ] && track_selected "$1"; then
    : "${GATE_SEAT_ASKED_AT:?track $1 runs in a build seat of its own: its launcher must set GATE_SEAT_ASKED_AT}"
    echo "track $1: build seat after $(($(date +%s) - GATE_SEAT_ASKED_AT)) s"
  fi
  [ -n "${GATE_STEP_TMP:-}" ] || { GATE_STEP_TMP=$(mktemp -d); TRACK_TMP=$GATE_STEP_TMP; trap 'rm -rf "$TRACK_TMP"' EXIT; }
  [ "$1" != "$QUIET_TRACK" ] || ! track_selected "$1" || quiet_hold
  for entry in ${TRACKS[$1]}; do
    n=${entry%%:*} name=${entry#*:}
    case $name in
      featuremap) set -- just cargo-free-checks ;;
      keystone-smoke | hand-authored | extra-nextest | keystone-tour) set -- bash "$(dirname "$0")/gate-integrate-steps.sh" "$name" ;;
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

# Needs GATE and tip. The tracks and reruns inherit the shape env, and
# scripts/gate-timing.sh checks the gate's record of every cargo call before it builds.
gate_env() {
  [ "$(jj log -r @- --no-graph -T 'commit_id')" = "$tip" ] || { echo "WRONG TREE: @- is not the tip $tip"; exit 9; }
  [ -z "$(jj diff -r @ --summary)" ] || { echo "WRONG TREE: @ carries edits on top of the tip $tip"; exit 9; }
  mkdir -p target/gate-logs
  export HOLON_CARGO_RECORD=$PWD/target/gate-logs/$GATE-cargo-calls.jsonl
  # shellcheck source=scripts/cargo-shape-env.sh
  source "$(dirname "$0")/cargo-shape-env.sh" gate
  PBT_FEATURES=$(just --justfile "$(dirname "$0")/../justfile" --evaluate CANON_FEATURES)
  export PBT_FEATURES
}

if [ "${1:-}" = rerun ]; then
  [ "$#" -eq 4 ] || { echo "usage: $0 rerun <gate-id> <tip> <red gate job id>" >&2; exit 2; }
  [[ $3 =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || { echo "usage: <tip> is a full commit id, not '$3'" >&2; exit 2; }
  unset GATE_SELECT GATE_FULL
  export GATE=$2 HOLON_GATE_ID=$2 GATE_TIP=$3 GATE_CARDS=${GATE_CARDS:-${FLOW_CARD:-none}}
  tip=$3 red_job=$4
  [ -s "target/gate-logs/$GATE-cargo-calls.jsonl" ] || { echo "NO GATE RECORD: target/gate-logs/$GATE-cargo-calls.jsonl"; exit 2; }
  gate_env
  rows="$(dirname "${HOLON_BUILD_HOLD_LOG:-$HOME/.cache/holon-build/holds.jsonl}")/gate-steps.jsonl"
  read -ra red <<<"$(/usr/bin/python3 -c 'import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
red = sorted((r["step"], r["name"]) for r in rows if r.get("job") == sys.argv[2] and not r.get("skipped") and r["exit"] != 0)
print(" ".join(f"{n}:{name}" for n, name in red))' "$rows" "$red_job")"
  [ "${#red[@]}" -gt 0 ] || { echo "RERUN REFUSED: no red step of the gate job $red_job in $rows"; exit 2; }
  declare -A ids
  for entry in "${red[@]}"; do
    name=${entry#*:}
    case $name in keystone-smoke | hand-authored | extra-nextest | keystone-tour | windowed-gate) ;;
      *) echo "RERUN REFUSED: step $name cannot be rerun"; exit 2 ;;
    esac
    ids[$name]=$(/usr/bin/python3 ~/.claude/skills/orchestrator/scripts/gate-rerun.py "$rows" "$red_job" "$name") && [ -n "${ids[$name]}" ] \
      || { echo "RERUN REFUSED: red step $name of $red_job recorded no failed test ids"; exit 2; }
  done
  log=target/gate-logs/rerun-$(date -u +%Y%m%dT%H%M%SZ).log
  {
    echo "BASE: $(jj log -r main --no-graph -T commit_id)"
    echo "TREE: $tip"
    echo "STARTED: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "gate-integrate.sh rerun: $GATE, the failed tests of $red_job; rerun draws fresh cases"
    shape_env_header
    first=0
    for entry in "${red[@]}"; do
      n=${entry%%:*} name=${entry#*:} rc=0
      if [ "$name" = windowed-gate ]; then set -- env WINDOWED_GATE_UNITS="${ids[$name]}" just windowed-gate
      else set -- bash "$(dirname "$0")/gate-integrate-steps.sh" "$name"; fi
      GATE_ONLY_IDS=${ids[$name]} step "$n" "$name" "$@" || rc=$?
      [ "$first" != 0 ] || first=$rc
    done
    exit "$first"
  } 2>&1 | tee "$log"
  exit
fi

[ "$#" -eq 3 ] || { echo "usage: $0 <gate-id> \"<cards>\" <tip>" >&2; exit 2; }
[[ $3 =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || { echo "usage: <tip> is a full commit id, as flow's --tip names it, not '$3'" >&2; exit 2; }
export GATE=$1 GATE_CARDS=$2 HOLON_GATE_ID=$1 GATE_PID=$$ HOLON_BUILD_QUIET_HOLDER=$$ GATE_TIP=$3 GATE_JOB_ID=${FLOW_JOB_ID:-}
tip=$3

gate_env
DISK_FLOOR_GIB=$(just --justfile "$(dirname "$0")/../justfile" --evaluate GATE_DISK_FLOOR_GIB)
[ "$(df -g . | awk 'NR==2{print $4}')" -ge "$DISK_FLOOR_GIB" ] || { echo "DISK BELOW $DISK_FLOOR_GIB GiB"; exit 8; }
: >"$HOLON_CARGO_RECORD"
echo "gate-integrate.sh: $GATE (pid $$), tip $tip, cards: $GATE_CARDS"
shape_env_header

/usr/bin/python3 "$(dirname "$0")/target-gc.py" target --mark
GATE_STEP_TMP=$(mktemp -d); export GATE_STEP_TMP
trap 'rm -rf "$GATE_STEP_TMP"' EXIT
gate_step 1 fmt cargo fmt --all -- --check
gate_step 20 deny just deny
gate_step 23 machete just machete
gate_step 2 gate-compile just gate-compile
rm -f target/gate-logs/"$GATE"-step-*
pids=()
"$0" --track 0 & pids+=($!)
for t in $(seq 1 $((${#TRACKS[@]} - 1))); do
  track_selected "$t" || { "$0" --track "$t" & pids+=($!); continue; }
  # A track of its own takes a slot of its own and writes its own job record.
  env -u HOLON_BUILD_SLOT_HOLDER -u FLOW_JOB_STATUS -u FLOW_JOB_LOG FLOW_JOB_ID="${FLOW_JOB_ID:-$GATE}.track$t" GATE_SEAT_ASKED_AT="$(date +%s)" \
    ~/.claude/skills/orchestrator/scripts/with-build-slot.sh "$0" --track "$t" & pids+=($!)
done
fail="" red="" extra_red=""
for t in "${!pids[@]}"; do
  rc=0; wait "${pids[$t]}" || rc=$?
  [ "$rc" = 0 ] || { echo "RED TRACK $t: exit $rc"; fail=${fail:-$rc}; }
done

for entry in "${STEPS[@]}"; do
  n=${entry%%:*} name=${entry#*:}
  [ ! -f "$(step_file "$n" log)" ] || cat "$(step_file "$n" log)"
  rc=$(cat "$(step_file "$n" rc)" 2>/dev/null) || { echo "RED STEP [$n] $name: its track ended before it"; fail=${fail:-1}; continue; }
  [ "$rc" = 0 ] && continue
  # Under the runner a step red is one the record names; any other exit (78: no build slot) means the step did not run.
  if { [ "${GATE_FULL:-}" = 1 ] || [ "${GATE_SELECT:-}" = 1 ]; } && ! grep -qxF "red [$n] $name (exit $rc)" "$GATE_RECORD"; then
    echo "RED STEP [$n] $name: exit $rc without running, log $(step_file "$n" log)"; fail=${fail:-$rc}; continue
  fi
  echo "RED STEP [$n] $name: exit $rc, log $(step_file "$n" log)"
  if [ "$name" = extra-nextest ]; then extra_red=$rc; [ "$rc" != 12 ] || echo "EXTRA NEXTEST NOVEL RED"
  else red=${red:-$rc}; fi
done
# A red half ends the gate here, so it prints the breach summary gate-timing.sh would print.
[ -z "$fail$red$extra_red" ] || /usr/bin/python3 "$(dirname "$0")/timing-breaches.py" "$GATE" || echo "TIMING BREACHES NOT SUMMARIZED (exit $?)"
[ -z "$fail" ] || exit "$fail"
if [ -n "$red$extra_red" ]; then
  [ "${GATE_FULL:-}" != 1 ] && [ "${GATE_SELECT:-}" != 1 ] || exit 0
  exit "${red:-$extra_red}"
fi
echo "INTEGRATE GREEN"
echo "candidate tip: $tip"
