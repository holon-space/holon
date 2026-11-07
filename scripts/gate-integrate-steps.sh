#!/usr/bin/env bash
# The steps of scripts/gate-integrate.sh that are pipelines: `step` runs one
# command, so each runs as `gate-integrate-steps.sh <name>`.
#
# Needs GATE and PBT_FEATURES (the justfile's CANON_FEATURES). With GATE_STEP_ARGS set (a selected gate),
# extra-nextest runs only the tests that nextest filter set names. `rerun <filterset>`
# runs the extra nextest step on those tests alone.
set -euo pipefail

gate_nextest() {
  HOLON_PBT_TIMING=measure cargo nextest run --profile load-tolerant --no-fail-fast "$@" --workspace --features "$PBT_FEATURES"
}

keystone_smoke() {
  local log=target/gate-logs/$GATE-smoke.log classified=target/gate-logs/$GATE-smoke-classify.txt
  HOLON_PBT_TIMING=measure just keystone-smoke 2>&1 | tee "$log" && return
  bash scripts/keystone-known-reds.sh "$log" | tee "$classified"
  grep -q 'PASS-WITH-NOTE' "$classified" || { echo "KEYSTONE SMOKE NOVEL RED"; exit 11; }
  echo "keystone smoke: pass-with-note (known reds only)"
}
# The replay runs every case; the classifier judges each red case against the
# row the case names.
hand_authored() {
  local log=target/gate-logs/$GATE-hand.log classified=target/gate-logs/$GATE-hand-classify.txt
  just hand-authored 2>&1 | tee "$log" && return
  bash scripts/keystone-known-reds.sh "$log" | tee "$classified" || true
  grep -q 'PASS-WITH-NOTE' "$classified" || { echo "HAND-AUTHORED NOVEL RED"; exit 11; }
  echo "hand-authored: pass-with-note (known reds only)"
}
extra_nextest() {
  local log=target/gate-logs/$GATE-extra.log classified=target/gate-logs/$GATE-extra-classify.txt
  local rc=0 filter=()
  # A selected crate may hold no test of this profile: that is a pass, not a wrong filter.
  [ -z "${GATE_STEP_ARGS:-}" ] || filter=(-E "$GATE_STEP_ARGS" --no-tests=warn)
  mark=$(mktemp); trap 'rm -f "$mark"' EXIT
  if ! gate_nextest ${filter[@]+"${filter[@]}"} 2>&1 | tee "$log"; then
    bash scripts/keystone-known-reds.sh "$log" | tee "$classified" || true
    grep -q 'PASS-WITH-NOTE' "$classified" || rc=12
    [ $rc -ne 0 ] || echo "extra nextest: pass-with-note (known reds only)"
  fi
  ./scripts/check-target-leftovers.sh "$mark" 2>&1 | tee "target/gate-logs/$GATE-leftovers.log" || { echo "TARGET LEFTOVERS"; rc=13; }
  return $rc
}

case "${1:-}" in
  keystone-smoke) keystone_smoke ;;
  hand-authored) hand_authored ;;
  extra-nextest) extra_nextest ;;
  rerun) gate_nextest -E "$2" --no-tests=fail ;;
  *) echo "usage: $0 keystone-smoke|hand-authored|extra-nextest|rerun <filterset>" >&2; exit 2 ;;
esac
