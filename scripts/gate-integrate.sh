#!/usr/bin/env bash
# The first half of the landing gate: every gate step that judges no duration,
# at any load, in the integration checkout that gate-combine.sh prepared (the
# current directory). It also builds every binary `just timing-gate` runs, so
# scripts/gate-timing.sh compiles nothing.
#
# Usage, inside a build slot (`flow job run ... -- scripts/gate-integrate.sh ...`):
#   scripts/gate-integrate.sh <gate-id> "<cards>" <tip>
#   GATE_DISK_FLOOR_GIB  free disk the gate needs (default 130).
#
# After gate-compile the steps run at once in TRACKS, one build slot per track:
# no step reads another step's output, and every one tolerates load. Each step
# writes its own log and gate-steps.jsonl row; the logs follow in TRACKS order.
#
# Last stdout line on green: `candidate tip: <tip>`, the argument for
# scripts/gate-timing.sh.
# Exit 0: every step green. A red step before gate-compile stops the run with
# its exit code; 9 = the checkout is not <tip>, 8 = free disk below the floor.
# After gate-compile every step runs to its end, each red step is named, and
# the run exits with the code of the first red one in TRACKS order; a red extra
# nextest alone exits 12 or 13 after the information step.
set -euo pipefail

# Steps that are pipelines run as `"$0" --<name>`: `step` runs one command.
keystone_smoke() {
  local log=target/gate-logs/$GATE-smoke.log classified=target/gate-logs/$GATE-smoke-classify.txt
  just keystone-smoke 2>&1 | tee "$log" && return
  bash scripts/keystone-known-reds.sh "$log" | tee "$classified"
  grep -q 'PASS-WITH-NOTE' "$classified" || { echo "KEYSTONE SMOKE NOVEL RED"; exit 11; }
  echo "keystone smoke: pass-with-note (known reds only)"
}
# The run stops at its first red case, so a known red is rerun once and the
# rerun must be green.
hand_authored() {
  local log=target/gate-logs/$GATE-hand.log classified=target/gate-logs/$GATE-hand-classify.txt
  just hand-authored 2>&1 | tee "$log" && return
  bash scripts/keystone-known-reds.sh "$log" | tee "$classified" || true
  grep -q 'PASS-WITH-NOTE' "$classified" || { echo "HAND-AUTHORED NOVEL RED"; exit 11; }
  echo "hand-authored: known red, rerun once"
  just hand-authored 2>&1 | tee "target/gate-logs/$GATE-hand-rerun.log"
}
extra_nextest() {
  local log=target/gate-logs/$GATE-extra.log classified=target/gate-logs/$GATE-extra-classify.txt
  local rc=0
  mark=$(mktemp); trap 'rm -f "$mark"' EXIT
  if ! HOLON_PBT_TIMING=measure cargo nextest run --profile load-tolerant --no-fail-fast \
      --workspace --features "$PBT_FEATURES" 2>&1 | tee "$log"; then
    bash scripts/keystone-known-reds.sh "$log" | tee "$classified" || true
    grep -q 'PASS-WITH-NOTE' "$classified" || rc=12
    [ $rc -ne 0 ] || echo "extra nextest: pass-with-note (known reds only)"
  fi
  ./scripts/check-target-leftovers.sh "$mark" 2>&1 | tee "target/gate-logs/$GATE-leftovers.log" || { echo "TARGET LEFTOVERS"; rc=13; }
  return $rc
}
PBT_FEATURES="holon-integration-tests/pbt,holon-integration-tests/web-arm,holon-gpui/pbt"

# Balanced by step time, longest track first; the first track runs in the
# gate's own slot. A step is <n>:<name>, its number in gate-steps.jsonl.
TRACKS=(
  "13:hand-authored"
  "11:keystone-smoke 19:extra-nextest 12:loro-suite 14:projector-lag-lock 15:windowed-gate"
  "17:timing-gate-build 7:gate-arch 3:check-release-shape 4:check-frontend-wasm 5:check-dioxus-web-wasm 6:check-worker-wasm 8:arch-validate 9:featuremap 10:analyze-arch 18:guests-verify 22:keystone-tour"
)
read -ra STEPS <<<"${TRACKS[*]}"
step_file() { echo "target/gate-logs/$GATE-step-$1.$2"; }
track() {
  local entry n name rc
  for entry in ${TRACKS[$1]}; do
    n=${entry%%:*} name=${entry#*:}
    case $name in
      featuremap) set -- /usr/bin/python3 scripts/featuremap.py check ;;
      keystone-smoke | hand-authored | extra-nextest) set -- "$0" "--$name" ;;
      keystone-tour) set -- env HOLON_PBT_TIMING=measure "HOLON_PERF_BUDGET=${HOLON_PERF_BUDGET-1}" cargo test --workspace --features "$PBT_FEATURES" \
        --test keystone_tour -- --include-ignored --nocapture ;;
      *) set -- just "$name" ;;
    esac
    rc=0; step "$n" "$name" "$@" >"$(step_file "$n" log)" 2>&1 || rc=$?
    echo "$rc" >"$(step_file "$n" rc)"
    echo "step [$n] $name: exit $rc"
  done
}

# shellcheck source=/dev/null
source ~/.claude/skills/orchestrator/scripts/gate-step.sh
case "${1:-}" in
  --keystone-smoke) keystone_smoke; exit ;;
  --hand-authored) hand_authored; exit ;;
  --extra-nextest) extra_nextest; exit ;;
  --track) track "$2"; exit ;;
esac

[ "$#" -eq 3 ] || { echo "usage: $0 <gate-id> \"<cards>\" <tip>" >&2; exit 2; }
export GATE=$1 GATE_CARDS=$2 HOLON_GATE_ID=$1
tip=$3
# The gpui build script then ignores git, so a git or jj call between the two
# scripts cannot rebuild what gate-integrate.sh built.
export HOLON_BUILD_SHA=${tip:0:12}
DISK_FLOOR_GIB=${GATE_DISK_FLOOR_GIB:-130}

[ "$(jj log -r @- --no-graph -T 'commit_id')" = "$tip" ] || { echo "WRONG TREE: @- is not the tip $tip"; exit 9; }
[ -z "$(jj diff -r @ --summary)" ] || { echo "WRONG TREE: @ carries edits on top of the tip $tip"; exit 9; }
[ "$(df -g . | awk 'NR==2{print $4}')" -ge "$DISK_FLOOR_GIB" ] || { echo "DISK BELOW $DISK_FLOOR_GIB GiB"; exit 8; }

# A font-kit build script reruns when this changes: the direct cargo calls here
# must see the justfile's value, or `just timing-gate` rebuilds the gpui crates.
RUST_FONTCONFIG_DLOPEN=$(just --evaluate RUST_FONTCONFIG_DLOPEN)
export RUST_FONTCONFIG_DLOPEN

mkdir -p target/gate-logs
echo "gate-integrate.sh: $GATE (pid $$), tip $tip, cards: $GATE_CARDS"

/usr/bin/python3 "$(dirname "$0")/target-gc.py" target --mark
step 1 fmt cargo fmt --all -- --check
step 20 deny just deny
step 2 gate-compile just gate-compile
rm -f target/gate-logs/"$GATE"-step-*
pids=()
"$0" --track 0 & pids+=($!)
for t in $(seq 1 $((${#TRACKS[@]} - 1))); do
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

echo "== information only: org_suite (compare its reds with the known reds) =="
cargo nextest run -p holon-integration-tests --test org_suite --no-fail-fast --features pbt,web-arm || echo "ORG_SUITE RED"
[ -z "$extra_red" ] || exit "$extra_red"
echo "INTEGRATE GREEN"
echo "candidate tip: $tip"
