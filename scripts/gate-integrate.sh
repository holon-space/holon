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
# Last stdout line on green: `candidate tip: <tip>`, the argument for
# scripts/gate-timing.sh.
# Exit 0: every step green. A red step stops the run with that step's exit
# code; 9 = the checkout is not <tip>, 8 = free disk below the floor. A red
# extra nextest exits 12 or 13 after the information step.
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
case "${1:-}" in
  --keystone-smoke) keystone_smoke; exit ;;
  --hand-authored) hand_authored; exit ;;
  --extra-nextest) extra_nextest; exit ;;
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
# shellcheck source=/dev/null
source ~/.claude/skills/orchestrator/scripts/gate-step.sh

/usr/bin/python3 "$(dirname "$0")/target-gc.py" target --mark
step 1 fmt cargo fmt --all -- --check
step 20 deny just deny
step 2 gate-compile just gate-compile
step 3 check-release-shape just check-release-shape
step 4 check-frontend-wasm just check-frontend-wasm
step 5 check-dioxus-web-wasm just check-dioxus-web-wasm
step 6 check-worker-wasm just check-worker-wasm
step 7 gate-arch just gate-arch
step 8 arch-validate just arch-validate
step 9 featuremap /usr/bin/python3 scripts/featuremap.py check
step 10 analyze-arch just analyze-arch
step 11 keystone-smoke "$0" --keystone-smoke
step 12 loro-suite just loro-suite
step 17 timing-gate-build just timing-gate-build
step 13 hand-authored "$0" --hand-authored
step 14 projector-lag-lock just projector-lag-lock
step 15 windowed-gate just windowed-gate
step 18 guests-verify just guests-verify
extra_red=""
step 19 extra-nextest "$0" --extra-nextest || { extra_red=$?; echo "EXTRA NEXTEST NOVEL RED"; }

echo "== information only: org_suite (compare its reds with the known reds) =="
cargo nextest run -p holon-integration-tests --test org_suite --no-fail-fast --features pbt,web-arm || echo "ORG_SUITE RED"
[ -z "$extra_red" ] || exit "$extra_red"
echo "INTEGRATE GREEN"
echo "candidate tip: $tip"
