#!/usr/bin/env bash
# The `run` step of the landing-gate batch (see gh-kanban's `batch NAME`): the
# gate's steps in the integration checkout that gate-combine.sh prepared, which
# is the current directory.
#
# Usage: GATE_CARD=<card> scripts/gate-run.sh [--dry-run]
#   GATE_CARD  the card the gate job counts against (`flow job run --card`).
#   GATE_DISK_FLOOR_GIB  free disk the gate needs (default 130).
#   --dry-run  runs the guards, then prints the steps instead of running them.
#
# Outside a build slot it starts itself as a `flow job run --quiet-steps` job,
# streams the job's log and exits with the job's exit code. The job stops when
# this waiter dies, a SIGKILL included.
#
# Exit 0: every step green. A red step stops the gate with that step's exit
# code; 9 = the checkout is not the combined tip, 8 = free disk below the floor.
# A red extra nextest exits 1 after the information step.
set -euo pipefail

SCRIPTS=~/.claude/skills/orchestrator/scripts
WBS=$SCRIPTS/with-build-slot.sh
DISK_FLOOR_GIB=${GATE_DISK_FLOOR_GIB:-130}
PBT_FEATURES="holon-integration-tests/pbt,holon-integration-tests/web-arm,holon-gpui/pbt"

# A step of its own: `step` runs one command, and the smoke's known-red
# classification is a pipeline.
keystone_smoke() {
  local log=target/gate-logs/gate-smoke.log classified=target/gate-logs/gate-smoke-classify.txt
  just keystone-smoke 2>&1 | tee "$log" && return
  bash scripts/keystone-known-reds.sh "$log" | tee "$classified"
  grep -q 'PASS-WITH-NOTE' "$classified" || { echo "KEYSTONE SMOKE NOVEL RED"; exit 11; }
  echo "keystone smoke: pass-with-note (known reds only)"
}
if [ "${1:-}" = --keystone-smoke ]; then keystone_smoke; exit; fi

DRY=""
if [ "${1:-}" = --dry-run ]; then DRY=1; fi

[ -f target/gate-tip ] || { echo "WRONG TREE: no target/gate-tip in $PWD; run gate-combine.sh first"; exit 9; }
tip=$(cat target/gate-tip)
[ "$(jj log -r @- --no-graph -T 'commit_id')" = "$tip" ] || { echo "WRONG TREE: @- is not the combined tip $tip"; exit 9; }
[ -z "$(jj diff -r @ --summary)" ] || { echo "WRONG TREE: @ carries edits on top of the combined tip $tip"; exit 9; }
[ "$(df -g . | awk 'NR==2{print $4}')" -ge "$DISK_FLOOR_GIB" ] || { echo "DISK BELOW $DISK_FLOOR_GIB GiB"; exit 8; }

if [ -z "$DRY" ] && [ -z "${HOLON_BUILD_SLOT_HOLDER:-}" ]; then
  : "${GATE_CARD:?gate-run.sh: set GATE_CARD, the card the gate job counts against}"
  export GATE_WAITER=$$
  out=$(flow job run --queue --card "$GATE_CARD" --lane gate --kind gate --quiet-steps -- "$0" "$@")
  echo "$out"
  id=$(sed -n 's/^OUTCOME: waiting-for //p' <<<"$out")
  [ -n "$id" ] || { echo "gate-run.sh: flow job run printed no job id"; exit 1; }
  status="target/jobs/$id.status"
  tail -n +1 -F "target/jobs/$id.log" 2>/dev/null & tailer=$!
  until [ -f "$status" ]; do sleep "${GATE_WAIT_POLL_S:-10}"; done
  kill "$tailer"
  exit "$(/usr/bin/python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["exit"])' "$status")"
fi

if [ -n "${GATE_WAITER:-}" ]; then
  slot=$HOLON_BUILD_SLOT_HOLDER gate=$$
  ( while kill -0 "$gate" 2>/dev/null && kill -0 "$GATE_WAITER" 2>/dev/null; do sleep 30; done
    kill -0 "$gate" 2>/dev/null || exit 0
    echo "gate-run.sh: the waiter $GATE_WAITER is gone; stopping the gate job"; kill -TERM "$slot" ) &
fi

mkdir -p target/gate-logs
GATE="gate-${tip:0:12}"
GATE_CARDS="${GATE_CARDS:-${FLOW_PR_HEADS:?gate-run.sh: set GATE_CARDS or FLOW_PR_HEADS}}"
export HOLON_GATE_ID=$GATE
echo "gate-run.sh: $GATE (pid $$), tip $tip, heads: $GATE_CARDS"
if [ -n "$DRY" ]; then
  step() { local n=$1 name=$2; shift 2; echo "step $n $name: $*"; }
  run() { echo "run: $*"; }
else
  # shellcheck source=/dev/null
  source "$SCRIPTS/gate-step.sh"
  run() { "$@"; }
fi

run /usr/bin/python3 "$(dirname "$0")/target-gc.py" target --mark
step 1 fmt cargo fmt --all -- --check
step 2 deny just deny
step 3 gate-compile just gate-compile
step 4 check-release-shape just check-release-shape
step 5 check-frontend-wasm just check-frontend-wasm
step 6 check-dioxus-web-wasm just check-dioxus-web-wasm
step 7 check-worker-wasm just check-worker-wasm
step 8 gate-arch just gate-arch
step 9 arch-validate just arch-validate
step 10 featuremap /usr/bin/python3 scripts/featuremap.py check
step 11 analyze-arch just analyze-arch
step 12 keystone-smoke "$0" --keystone-smoke
step 13 loro-suite just loro-suite
step 14 guests-verify just guests-verify

# The timing steps must not compile under the quiet hold.
run cargo test --workspace --features "$PBT_FEATURES" --no-run
run cargo nextest run --no-run --workspace --features holon/test-helpers

# The hold lowers every other build, so it spans the timing steps only.
run "$WBS" --quiet-hold $$
export HOLON_BUILD_QUIET_HOLDER=$$
[ -n "$DRY" ] || trap '"$WBS" --quiet-release $$' EXIT
step 15 hand-authored just hand-authored
step 16 projector-lag-lock just projector-lag-lock
step 17 windowed-gate just windowed-gate
step 18 latency-slo-gate just latency-slo-gate
step 19 org-deep-text-timing just org-deep-text-timing
extra_red=""
step 20 extra-nextest cargo nextest run --no-fail-fast --workspace --features holon/test-helpers || { echo "EXTRA NEXTEST RED"; extra_red=1; }
run "$WBS" --quiet-release $$
unset HOLON_BUILD_QUIET_HOLDER
trap - EXIT
[ -n "$extra_red" ] || echo "GATE ALL GREEN"

echo "== information only: org_suite (compare its reds with the known reds) =="
run cargo nextest run -p holon-integration-tests --test org_suite --no-fail-fast --features pbt,web-arm || echo "ORG_SUITE RED"
run /usr/bin/python3 "$(dirname "$0")/target-gc.py" target --apply --incremental-below "$DISK_FLOOR_GIB" || echo "TARGET-GC NOT RUN (exit $?), see above"
[ -z "$extra_red" ]
