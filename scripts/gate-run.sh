#!/usr/bin/env bash
# The `run` step of the landing-gate batch (see gh-kanban's `batch NAME`): the
# gate's steps in the integration checkout that gate-combine.sh prepared, which
# is the current directory.
#
# Usage: scripts/gate-run.sh [--dry-run]
#   --dry-run  runs the guards, then prints the steps instead of running them.
#
# Exit 0: every step green. A red step stops the gate with that step's exit
# code; 9 = the checkout is not the combined tip, 8 = free disk below the floor.
# A red extra nextest exits 1 after the information step.
set -euo pipefail

SCRIPTS=~/.claude/skills/orchestrator/scripts
WBS=$SCRIPTS/with-build-slot.sh
DISK_FLOOR_GIB=130
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
  HOLON_BUILD_TAKES_QUIET_HOLD=1 exec "$WBS" "$0" "$@"
fi

mkdir -p target/gate-logs
GATE="gate-${tip:0:12}"
GATE_CARDS="${GATE_CARDS:-${FLOW_PR_HEADS:?gate-run.sh: set GATE_CARDS or FLOW_PR_HEADS}}"
export HOLON_GATE_ID=$GATE
echo "gate-run.sh: $GATE, tip $tip, heads: $GATE_CARDS"
if [ -n "$DRY" ]; then
  step() { local n=$1 name=$2; shift 2; echo "step $n $name: $*"; }
  run() { echo "run: $*"; }
else
  # shellcheck source=/dev/null
  source "$SCRIPTS/gate-step.sh"
  run() { "$@"; }
fi

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

# The timing steps must not compile under the quiet hold.
run cargo test --workspace --features "$PBT_FEATURES" --no-run
run cargo nextest run --no-run --workspace --features holon/test-helpers

run "$WBS" --quiet-hold $$
export HOLON_BUILD_QUIET_HOLDER=$$
[ -n "$DRY" ] || trap '"$WBS" --quiet-release $$' EXIT
step 14 hand-authored just hand-authored
step 15 projector-lag-lock just projector-lag-lock
step 16 windowed-gate just windowed-gate
step 17 latency-slo-gate just latency-slo-gate
step 18 org-deep-text-timing just org-deep-text-timing
step 19 guests-verify just guests-verify
extra_red=""
step 20 extra-nextest cargo nextest run --no-fail-fast --workspace --features holon/test-helpers || { echo "EXTRA NEXTEST RED"; extra_red=1; }
[ -n "$extra_red" ] || echo "GATE ALL GREEN"

echo "== information only: org_suite (compare its reds with the known reds) =="
run cargo nextest run -p holon-integration-tests --test org_suite --no-fail-fast --features pbt,web-arm || echo "ORG_SUITE RED"
[ -z "$extra_red" ]
