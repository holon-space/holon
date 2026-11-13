#!/usr/bin/env bash
# The steps of scripts/gate-integrate.sh that are pipelines: `step` runs one
# command, so each runs as `gate-integrate-steps.sh <name>`.
#
# Needs GATE and PBT_FEATURES (the justfile's CANON_FEATURES). With GATE_STEP_ARGS set (a selected gate),
# extra-nextest runs only the tests that nextest filter set names; a filter set nextest
# cannot parse ends the step with exit 2 before any test runs.
# A red step writes its failed test ids to $GATE_IDS_FILE (scripts/gate-failed-ids.py names
# their grammar); with GATE_ONLY_IDS set (a rerun, one id per line) a step runs exactly those
# tests, through the same known-reds classifier. An id the step cannot run ends it with exit 2.
set -euo pipefail

here=$(dirname "$0")
only=()
if [ -n "${GATE_ONLY_IDS:-}" ]; then while IFS= read -r id; do only+=("$id"); done <<<"$GATE_ONLY_IDS"; fi

gate_nextest() {
  HOLON_PBT_TIMING=measure cargo nextest run --profile load-tolerant --no-fail-fast "$@" --workspace --features "$PBT_FEATURES"
}
cannot_rerun() { echo "$1: cannot rerun the id '$2'"; exit 2; }
record_failed() {
  [ -n "${GATE_IDS_FILE:-}" ] || return 0
  local ids
  ids=$(/usr/bin/python3 "$here/gate-failed-ids.py" "$@") || { echo "$1: failed test ids not recorded: gate-failed-ids.py exit $?"; exit 2; }
  [ -z "$ids" ] || sed 's/^/failed /' <<<"$ids" >>"$GATE_IDS_FILE"
}

keystone_smoke() {
  local log=target/gate-logs/$GATE-smoke.log classified=target/gate-logs/$GATE-smoke-classify.txt
  local shape tests=() rerun_shape="" id rc=0 r
  shape=$(just --justfile "$here/../justfile" --evaluate SHAPE_CASE)
  for id in ${only[@]+"${only[@]}"}; do
    case $id in
      "test "*) tests+=("${id#test }") ;;
      "case $shape") rerun_shape=1 ;;
      *) cannot_rerun keystone-smoke "$id" ;;
    esac
  done
  if [ "${#only[@]}" = 0 ]; then
    HOLON_PBT_TIMING=measure just keystone-smoke 2>&1 | tee "$log" || rc=$?
  else
    : >"$log"
    [ "${#tests[@]}" = 0 ] || HOLON_PBT_TIMING=measure just pbt general 1 --exact "${tests[@]}" 2>&1 | tee -a "$log" || rc=$?
    [ -z "$rerun_shape" ] || HOLON_PBT_TIMING=measure just keystone-smoke-shape 2>&1 | tee -a "$log" || { r=$?; [ "$rc" != 0 ] || rc=$r; }
  fi
  [ "$rc" != 0 ] || return 0
  bash scripts/keystone-known-reds.sh "$log" | tee "$classified"
  grep -q 'PASS-WITH-NOTE' "$classified" || { record_failed keystone-smoke "$log" "$shape"; echo "KEYSTONE SMOKE NOVEL RED"; exit 11; }
  echo "keystone smoke: pass-with-note (known reds only)"
}
# The replay runs every case; the classifier judges each red case against the
# row the case names.
hand_authored() {
  local log=target/gate-logs/$GATE-hand.log classified=target/gate-logs/$GATE-hand-classify.txt
  local id cases=""
  for id in ${only[@]+"${only[@]}"}; do
    [[ $id =~ ^[a-z0-9-]+$ ]] || cannot_rerun hand-authored "$id"
    cases+="${cases:+,}$id"
  done
  env ${cases:+HOLON_HAND_AUTHORED_CASE="$cases"} just hand-authored 2>&1 | tee "$log" && return
  bash scripts/keystone-known-reds.sh "$log" | tee "$classified" || true
  grep -q 'PASS-WITH-NOTE' "$classified" || { record_failed hand-authored "$log"; echo "HAND-AUTHORED NOVEL RED"; exit 11; }
  echo "hand-authored: pass-with-note (known reds only)"
}
extra_nextest() {
  local log=target/gate-logs/$GATE-extra.log classified=target/gate-logs/$GATE-extra-classify.txt
  local rc=0 filter=() id set=""
  for id in ${only[@]+"${only[@]}"}; do
    [[ $id =~ ^([^\ (),]+)\ ([^\ (),]+)$ ]] || cannot_rerun extra-nextest "$id"
    set+="${set:+ | }(binary_id(=${BASH_REMATCH[1]}) & test(=${BASH_REMATCH[2]}))"
  done
  if [ -n "$set" ]; then
    filter=(-E "$set" --no-tests=fail)
  elif [ -n "${GATE_STEP_ARGS:-}" ]; then
    # The joined form keeps args that start with `-` from reading as nextest options.
    filter=(--filterset="$GATE_STEP_ARGS" --no-tests=fail)
  fi
  mark=$(mktemp); trap 'rm -f "$mark"' EXIT
  if gate_nextest ${filter[@]+"${filter[@]}"} 2>&1 | tee "$log"; then :; else
    local status=("${PIPESTATUS[@]}")
    # nextest exits 94 on a command-line filter set it cannot parse, before it builds.
    if [ -n "${GATE_STEP_ARGS:-}" ] && [ "${status[0]}" = 94 ]; then
      echo "extra-nextest: GATE_STEP_ARGS is no valid filterset: $GATE_STEP_ARGS"
      return 2
    fi
    bash scripts/keystone-known-reds.sh "$log" | tee "$classified" || true
    grep -q 'PASS-WITH-NOTE' "$classified" || rc=12
    [ $rc -ne 0 ] || echo "extra nextest: pass-with-note (known reds only)"
  fi
  ./scripts/check-target-leftovers.sh "$mark" 2>&1 | tee "target/gate-logs/$GATE-leftovers.log" || { echo "TARGET LEFTOVERS"; rc=13; }
  [ $rc != 12 ] || record_failed extra-nextest "${CARGO_TARGET_DIR:-target}/nextest/load-tolerant/junit.xml" "$mark"
  return $rc
}
keystone_tour() {
  local log=target/gate-logs/$GATE-tour.log id tests=()
  for id in ${only[@]+"${only[@]}"}; do
    [[ $id =~ ^holon-integration-tests::integration\ (keystone_tour::[^\ ]+)$ ]] || cannot_rerun keystone-tour "$id"
    tests+=("${BASH_REMATCH[1]}")
  done
  [ "${#tests[@]}" = 0 ] || tests=(--exact "${tests[@]}")
  local rc=0
  just keystone-tour ${tests[@]+"${tests[@]}"} 2>&1 | tee "$log" || rc=$?
  [ "$rc" = 0 ] || record_failed keystone-tour "$log"
  return "$rc"
}

case "${1:-}" in
  keystone-smoke) keystone_smoke ;;
  hand-authored) hand_authored ;;
  extra-nextest) extra_nextest ;;
  keystone-tour) keystone_tour ;;
  *) echo "usage: $0 keystone-smoke|hand-authored|extra-nextest|keystone-tour" >&2; exit 2 ;;
esac
